use std::collections::BTreeMap;
use std::path::PathBuf;
use thiserror::Error;
use tracing::{error, info, instrument};

use crate::github::{GitHubRepo, GithubApiClient};
use crate::models::{CreatedPullRequest, MigrationDefinition};
use crate::steps::MigrationStep;
use crate::steps::{
    CloneRepoStep, MigrationScriptStep, MigrationStepResult, PreFlightCheckStep, PushRepoStep,
    UpdateGithubStep,
};
use crate::workspace::Workspace;

#[derive(Debug)]
pub struct ExecutionOptions<'a> {
    pub skip_pull_request: bool,
    pub skip_push: bool,
    pub dry_run: bool,
    pub env: BTreeMap<String, String>,
    pub work_dir: PathBuf,
    pub github_client: &'a GithubApiClient,
}

impl<'a> ExecutionOptions<'a> {
    fn is_push_enabled(&self) -> bool {
        !self.dry_run && !self.skip_push
    }

    fn is_pr_enabled(&self) -> bool {
        !self.dry_run && !self.skip_pull_request
    }
}

#[derive(Error, Debug)]
pub enum MigrationError {
    #[error("Unable to checkout {repo}. Got error: {source:?}")]
    UnableToCheckoutRepo {
        repo: String,
        #[source]
        source: anyhow::Error,
    },
    #[error("Unable to creat pull request.")]
    UnableToCreatePullRequest {
        #[source]
        source: anyhow::Error,
    },
    #[error("Unable to parse Git Repo.")]
    InvalidGitRepo {
        #[source]
        source: crate::github::GitHubError,
    },
    #[error("Migration determined that repo was not eligible for migration.")]
    MigrationNotRequired,
    #[error("Migration step `{step_name}` exited non-zero.")]
    MigrationStepErrored { step_name: String },
    #[error("Migration step `{step_name}` left working directory had untracked filed: {files:?}.")]
    WorkingDirNotClean {
        step_name: String,
        files: Vec<String>,
    },
    #[error(transparent)]
    IoError(#[from] std::io::Error),
    #[error(transparent)]
    AnyHowError(#[from] anyhow::Error),
    #[error(transparent)]
    GitError(#[from] git2::Error),
    #[error(transparent)]
    CommandError(#[from] crate::workspace::CommandError),
}

#[derive(Debug)]
pub enum MigrationStatus {
    EmptyResponse(MigrationStepResult<()>),
    PullRequest(MigrationStepResult<CreatedPullRequest>),
}

#[derive(Debug)]
pub struct MigrationTask<'a> {
    pub pretty_name: String,
    pub repo: GitHubRepo,
    pub definition: MigrationDefinition,
    pub exec_opts: ExecutionOptions<'a>,
    pub pull_request: Option<CreatedPullRequest>,
    pub skip: bool,
}

impl<'a> MigrationTask<'a> {
    pub fn new<S: Into<String>>(
        pretty_name: S,
        repo: GitHubRepo,
        definition: MigrationDefinition,
        exec_opts: ExecutionOptions<'a>,
        pull_request: Option<CreatedPullRequest>,
        skip: bool,
    ) -> Self {
        Self {
            pretty_name: pretty_name.into(),
            repo,
            definition,
            exec_opts,
            pull_request,
            skip
        }
    }

    #[instrument(name = "migrate", skip(self), fields(name = %self.pretty_name))]
    pub async fn run(&self) -> MigrationStatus {
        if self.skip {
            return MigrationStatus::EmptyResponse(MigrationStepResult::abort("skip"));
        }
        
        let work_dir = match self.exec_opts.work_dir.canonicalize() {
            Ok(dir) => dir,
            Err(e) => {
                error!("Unable to canonicalize dir: {:?}", e);
                return MigrationStatus::EmptyResponse(MigrationStepResult::failure(
                    "init",
                    MigrationError::IoError(e),
                ));
            }
        };

        info!("Processing {} in {:?}", self.pretty_name, work_dir);
        let mut workspace = match Workspace::new(&self.pretty_name, &work_dir) {
            Ok(dir) => dir,
            Err(e) => {
                error!("Unable to create workspace: {:?}", e);
                return MigrationStatus::EmptyResponse(MigrationStepResult::failure(
                    "init",
                    MigrationError::IoError(e),
                ));
            }
        };

        let status = CloneRepoStep::from(self).execute_step(&mut workspace).await;
        if status.terminal {
            return MigrationStatus::EmptyResponse(status);
        }

        let status = PreFlightCheckStep::from(self)
            .execute_step(&mut workspace)
            .await;
        if status.terminal {
            return MigrationStatus::EmptyResponse(status);
        }

        for step in &self.definition.steps {
            let status = MigrationScriptStep::from(step)
                .execute_step(&mut workspace)
                .await;
            if status.terminal {
                return MigrationStatus::EmptyResponse(status);
            }
        }

        if self.exec_opts.is_push_enabled() {
            let status = PushRepoStep::new().execute_step(&mut workspace).await;
            if status.terminal {
                return MigrationStatus::EmptyResponse(status);
            }

            if self.exec_opts.is_pr_enabled() {
                return MigrationStatus::PullRequest(
                    UpdateGithubStep::from(self)
                        .execute_step(&mut workspace)
                        .await,
                );
            } else {
                MigrationStatus::EmptyResponse(MigrationStepResult::abort("pull-request"))
            }
        } else {
            MigrationStatus::EmptyResponse(MigrationStepResult::abort("push"))
        }
    }
}
