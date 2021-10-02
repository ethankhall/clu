use anyhow::Result as AnyResult;
use async_trait::async_trait;
use git2::Repository;
use tracing::{info, instrument};

use super::{MigrationStep, MigrationStepResult};
use crate::github::GitHubRepo;
use crate::migration::{MigrationError, MigrationTask};
use crate::workspace::Workspace;

pub struct CloneRepoStep<'a> {
    branch_name: &'a str,
    repo: &'a GitHubRepo,
}

#[async_trait]
impl<'a> MigrationStep<()> for CloneRepoStep<'a> {
    #[instrument(name = "clone", skip(self, workspace), fields(workspace_name = %workspace.workspace_name, repo = %self.repo))]
    async fn execute_step(&self, workspace: &mut Workspace) -> MigrationStepResult<()> {
        match self.clone_repo(workspace).await {
            Ok(_) => MigrationStepResult::success("clone"),
            Err(e) => MigrationStepResult::failure(
                "clone",
                MigrationError::UnableToCheckoutRepo {
                    repo: format!("{}/{}", self.repo.owner, self.repo.repo),
                    source: e,
                },
            ),
        }
    }
}

impl<'a> CloneRepoStep<'a> {
    pub fn new(branch_name: &'a str, repo: &'a GitHubRepo) -> Self {
        Self { branch_name, repo }
    }

    async fn clone_repo(&self, workspace: &mut Workspace) -> AnyResult<()> {
        let git_repo = workspace.root_dir.join("repo");

        info!(
            "Cloning {} into {}",
            workspace.workspace_name,
            git_repo.to_str().unwrap()
        );

        workspace
            .run_command_successfully(&format!(
                "git clone {} {}",
                &self.repo.clone_url,
                git_repo.to_str().unwrap()
            ))
            .await?;
        workspace.set_working_dir("repo");

        info!("Creating {} branch", &self.branch_name);
        let repo = Repository::open(git_repo.to_str().unwrap())?;
        repo.branch(self.branch_name, &repo.head()?.peel_to_commit()?, true)?;

        repo.config()?.set_str("push.default", "current")?;

        let obj = repo.revparse_single(&format!("refs/heads/{}", self.branch_name))?;

        repo.checkout_tree(&obj, None)?;

        repo.set_head(&format!("refs/heads/{}", self.branch_name))?;

        Ok(())
    }
}

impl<'a> From<&'a MigrationTask<'_>> for CloneRepoStep<'a> {
    fn from(task: &'a MigrationTask) -> Self {
        Self::new(&task.definition.checkout.branch_name, &task.repo)
    }
}

pub struct PushRepoStep {}

impl PushRepoStep {
    pub fn new() -> Self {
        Self {}
    }
}

impl Default for PushRepoStep {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl MigrationStep<()> for PushRepoStep {
    #[instrument(name = "push", skip(self, workspace), fields(workspace_name = %workspace.workspace_name))]
    async fn execute_step(&self, workspace: &mut Workspace) -> MigrationStepResult<()> {
        match workspace
            .run_command_successfully("git push --force-with-lease")
            .await
        {
            Err(e) => MigrationStepResult::failure("push", MigrationError::CommandError(e)),
            Ok(_) => MigrationStepResult::success("push"),
        }
    }
}

pub struct RepoCheck {}

impl RepoCheck {
    pub async fn check_for_untracked_files(
        step_name: &str,
        workspace: &mut Workspace,
    ) -> Result<(), MigrationError> {
        let git_repo = workspace.root_dir.join("repo");

        let repo = Repository::open(git_repo)?;
        let status = repo.statuses(None)?;
        if !status.is_empty() {
            let files: Vec<String> = status
                .iter()
                .map(|x| x.path().unwrap().to_owned())
                .collect();

            return Err(MigrationError::WorkingDirNotClean {
                step_name: step_name.to_owned(),
                files,
            });
        }

        Ok(())
    }
}
