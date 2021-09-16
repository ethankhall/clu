use crate::models::PullRequest;
use anyhow::Result as AnyResult;
use hubcaps::pulls::{PullEditOptions, PullOptions};
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

#[derive(Debug, PartialEq, Eq)]
pub struct GitHubRepo {
    pub owner: String,
    pub repo: String,
}

impl GitHubRepo {
    fn new<G: Into<String>>(owner: G, repo: G) -> Self {
        Self { owner: owner.into(), repo: repo.into() }
    }
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
            let owner = matches.name("owner").unwrap().as_str();
            let repo = matches.name("repo").unwrap().as_str();

            Ok(GitHubRepo::new(owner, repo))
        }
        None => Err(GitHubError::UnableToDetermineRepo {
            path: url.to_owned(),
        }),
    }
}

#[test]
fn validate_extract_github_info() {
  assert_eq!(GitHubRepo::new("ethankhall", "clu"), extract_github_info("https://github.com/ethankhall/clu").unwrap());
  assert_eq!(GitHubRepo::new("ethankhall", "clu"), extract_github_info("https://github.com/ethankhall/clu.git").unwrap());  
  assert_eq!(GitHubRepo::new("ethankhall", "clu"), extract_github_info("git@github.com:ethankhall/clu.git").unwrap());    
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

    let pulls = repo.pulls();
    info!("Creating PR for {}", &create_pr.repo);

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

pub async fn update_pull_request(
    github_token: &str,
    pr_number: u64,
    create_pr: CreatePullRequest<'_>,
) -> Result<u64, GitHubError> {
    let github = Github::new(
        format!("clu/{}", env!("CARGO_PKG_VERSION")),
        Credentials::Token(github_token.to_owned()),
    )?;

    let repo = github.repo(create_pr.repo.owner.clone(), create_pr.repo.repo.clone());
    info!("Getting repo details for {}", &create_pr.repo);

    let pulls = repo.pulls();

    let existing_pr = pulls.get(pr_number);
    let actual_pr = existing_pr.get().await?;

    if actual_pr.state != "open" {
        info!("PR ({}) was not open, making a new one", actual_pr.url);
        return create_pull_request(github_token, create_pr).await
    }
    info!("Updating PR for {}", &create_pr.repo);
    let updated_pr = existing_pr
        .edit(&PullEditOptions::builder().title(create_pr.title).body(create_pr.body).build())
        .await?;

    info!("Updated PR {}", updated_pr.url);

    Ok(updated_pr.number)
}
