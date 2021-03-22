use crate::models::PullRequest;
use anyhow::Result as AnyResult;
use hubcaps::pulls::PullOptions;
use hubcaps::{Credentials, Github};
use regex::Regex;
use std::fmt;
use thiserror::Error;
use tracing::{debug, info};

pub struct CreatePullRequest<'a> {
    pub repo: &'a GitHubRepo,
    pub branch: &'a str,
    pub title: &'a str,
    pub body: &'a str,
}

pub struct GitHubRepo {
    pub owner: String,
    pub repo: String,
}

impl fmt::Display for GitHubRepo {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}/{}", self.owner, self.repo)
    }
}

#[derive(Error, Debug)]
pub enum GitHubError {
    #[error("Unable to determine GitHub owner/repo from {path}")]
    UnableToDetermineRepo { path: String },
    #[error(transparent)]
    HubcapError(#[from] hubcaps::Error),
}

pub enum PullStatus {
    ChecksFailed,
    NeedsApproval,
    Mergeable,
    Merged,
}

pub async fn fetch_pull_status(github_token: &str, pull: &PullRequest) -> AnyResult<PullStatus> {
    let github = Github::new(
        format!("clu/{}", env!("CARGO_PKG_VERSION")),
        Credentials::Token(github_token.to_owned()),
    )?;

    let repo = github.repo(pull.owner.clone(), pull.repo.clone());
    let pulls = repo.pulls();
    let gh_pr = pulls.get(pull.pr_number).get().await?;

    let statuses = repo.statuses().list(&gh_pr.head.sha).await?;
    debug!("Statuses {:?}", statuses);

    if gh_pr.merged {
        return Ok(PullStatus::Merged);
    }

    let checks_pass = statuses
        .iter()
        .all(|x| x.state == hubcaps::statuses::State::Success);
    if checks_pass {
        return Ok(PullStatus::ChecksFailed);
    }

    if gh_pr.mergeable == Some(true) {
        Ok(PullStatus::Mergeable)
    } else {
        Ok(PullStatus::NeedsApproval)
    }
}

pub fn extract_github_info(url: &str) -> Result<GitHubRepo, GitHubError> {
    let re =
        Regex::new("^(https://github.com/|git@github.com:)(?P<owner>.+?)/(?P<repo>.+?)(\\.git)?$")
            .unwrap();

    match re.captures(url) {
        Some(matches) => {
            let owner = matches.name("owner").unwrap().as_str().to_string();
            let repo = matches.name("repo").unwrap().as_str().to_string();

            Ok(GitHubRepo { owner, repo })
        }
        None => Err(GitHubError::UnableToDetermineRepo {
            path: url.to_owned(),
        }),
    }
}

pub async fn create_pull_request(
    github_token: &str,
    create_pr: CreatePullRequest<'_>,
) -> Result<u64, GitHubError> {
    let github = Github::new(
        format!("clu/{}", env!("CARGO_PKG_VERSION")),
        Credentials::Token(github_token.to_owned()),
    )?;

    let repo = github.repo(create_pr.repo.owner.clone(), create_pr.repo.repo.clone());
    info!("Getting repo details for {}", &create_pr.repo);
    let repo_details = repo.get().await?;

    info!("Creating PR for {}", &create_pr.repo);

    let pulls = repo.pulls();
    let created = pulls
        .create(&PullOptions {
            title: create_pr.title.to_owned(),
            head: create_pr.branch.to_owned(),
            base: repo_details.default_branch,
            body: Some(create_pr.body.to_owned()),
        })
        .await?;

    info!("Created PR {}", created.url);

    Ok(created.number)
}
