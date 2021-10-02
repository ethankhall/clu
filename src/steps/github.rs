use async_trait::async_trait;
use tracing::instrument;

use super::{MigrationStep, MigrationStepResult};
use crate::github::{GitHubRepo, GithubApiClient, PullRequestDescription};
use crate::migration::{MigrationError, MigrationTask};
use crate::models::CreatedPullRequest;
use crate::workspace::Workspace;

pub struct UpdateGithubStep<'a> {
    github_api: &'a GithubApiClient,
    repo: &'a GitHubRepo,
    existing_pr: Option<CreatedPullRequest>,
    branch: &'a str,
    title: &'a str,
    body: &'a str,
}

#[async_trait]
impl<'a> MigrationStep<CreatedPullRequest> for UpdateGithubStep<'a> {
    #[instrument(name = "pull-request", skip(self, _workspace), fields(workspace_name = %_workspace.workspace_name, repo = %self.repo))]
    async fn execute_step(
        &self,
        _workspace: &mut Workspace,
    ) -> MigrationStepResult<CreatedPullRequest> {
        match self
            .github_api
            .sync_pull_request(
                self.repo,
                PullRequestDescription {
                    branch: self.branch,
                    title: self.title,
                    body: self.body,
                },
                self.existing_pr.as_ref().map(|it| it.pr_number),
            )
            .await
        {
            Err(e) => MigrationStepResult::failure(
                "pull-request",
                MigrationError::UnableToCreatePullRequest { source: e },
            ),
            Ok(new_pr) => {
                let pr = CreatedPullRequest {
                    pr_number: new_pr.number,
                    url: new_pr.permalink,
                };
                MigrationStepResult::success_with_result("pull-request", pr)
            }
        }
    }
}

impl<'a> From<&'a MigrationTask<'a>> for UpdateGithubStep<'a> {
    fn from(task: &'a MigrationTask) -> Self {
        Self {
            github_api: task.exec_opts.github_client,
            repo: &task.repo,
            existing_pr: task.pull_request.clone(),
            branch: &task.definition.checkout.branch_name,
            title: &task.definition.pr.title,
            body: &task.definition.pr.description,
        }
    }
}
