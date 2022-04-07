use clap::Clap;

use std::collections::BTreeMap;
use std::fs::read_to_string;
use std::path::PathBuf;

use anyhow::Result as AnyResult;
use futures::stream::{self, StreamExt};
use tracing::{info, warn};

use crate::github::GithubApiClient;
use crate::github::PullStatus;
use crate::migration::MigrationError;
use crate::models::*;
use crate::steps::FollowUpStep;
use crate::steps::{MigrationStep, MigrationStepResult};
use crate::workspace::Workspace;

#[derive(Clap, Debug)]
pub struct RunFollowupArgs {
    /// A TOML file that defines the input needed to run a migration. This file will be updated
    /// with the results of the run.
    #[clap(long)]
    pub migration_definition: String,

    /// Token to be used when talking to GitHub
    #[clap(long, env = "GITHUB_TOKEN")]
    pub github_token: String,

    /// Folder where the work will take place
    #[clap(long = "work-directory", default_value("follow-up-dir"))]
    pub work_directory_root: String,

    pub followup_script: String,
}

pub async fn run_followup(args: RunFollowupArgs) -> AnyResult<()> {
    let results: MigrationFile = toml::from_str(&read_to_string(args.migration_definition)?)?;

    let github_api = GithubApiClient::new(&args.github_token)?;

    let mut work_queue = Vec::new();

    for (name, target) in results.targets {
        let target_dir = PathBuf::from(&args.work_directory_root);

        let pull = match target.pull_request {
            Some(pull) => pull,
            _ => continue,
        };

        work_queue.push(WorkTask {
            repo_name: name,
            github_api: &github_api,
            pull,
            clone_url: target.repo,
            target_dir: target_dir.clone(),
            followup_script: args.followup_script.clone(),
        });
    }

    stream::iter(work_queue)
        .for_each_concurrent(3, |task| async move {
            let migration_status = task.run_follow_up().await;
            match migration_status.result {
                Ok(_) => info!("{} ran follow up successfully", task.repo_name),
                Err(e) => warn!(
                    "{} did not run follow-up successfully: {:?}",
                    task.repo_name, e
                ),
            }
        })
        .await;

    Ok(())
}

struct WorkTask<'a> {
    repo_name: String,
    github_api: &'a GithubApiClient,
    pull: CreatedPullRequest,
    clone_url: String,
    target_dir: PathBuf,
    followup_script: String,
}

impl<'a> WorkTask<'a> {
    async fn run_follow_up(&self) -> MigrationStepResult<()> {
        let github_repo = match crate::github::extract_github_info(&self.clone_url) {
            Ok(github_repo) => github_repo,
            Err(e) => {
                return MigrationStepResult::failure(
                    "invalid-url",
                    MigrationError::InvalidGitRepo { source: e },
                )
            }
        };

        let pr_state = match self
            .github_api
            .fetch_pull_state(&github_repo, self.pull.pr_number)
            .await
        {
            Ok(pr_state) => pr_state,
            Err(e) => {
                warn!(
                    "Unable to get pull request {}: {:?}",
                    self.pull.pr_number, e
                );
                return MigrationStepResult::failure(
                    "no-pull-request",
                    MigrationError::AnyHowError(e),
                );
            }
        };

        if pr_state.status == PullStatus::Merged {
            MigrationStepResult::abort("merged");
        }

        let mut env_vars = BTreeMap::new();
        env_vars.insert("CLU_PULL_REQUEST_URL".to_owned(), pr_state.permalink);
        env_vars.insert("CLU_CLONE_URL".to_owned(), self.clone_url.to_owned());

        let mut workspace =
            match Workspace::new_clean_workspace(&self.repo_name, self.target_dir.as_path()) {
                Ok(workspace) => workspace,
                Err(e) => {
                    return MigrationStepResult::failure("workspace", MigrationError::IoError(e))
                }
            };
        workspace.set_env_vars(&mut env_vars);
        FollowUpStep::new(&self.followup_script)
            .execute_step(&mut workspace)
            .await
    }
}
