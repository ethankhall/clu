use crate::models::CreatedPullRequest;
use anyhow::{bail, Result as AnyResult};
use regex::Regex;
use std::fmt;
use thiserror::Error;
use tracing::{debug, info};
use graphql_client::GraphQLQuery;
use reqwest::Client;

#[allow(clippy::upper_case_acronyms)]
type URI = String;

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "src/graphql/schema.docs.graphql",
    query_path = "src/graphql/GetPullRequestStatusQuery.graphql",
    response_derives = "Debug,PartialEq"
)]
pub struct GetPullRequestStatusQuery;

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "src/graphql/schema.docs.graphql",
    query_path = "src/graphql/CreatePullRequest.graphql",
    response_derives = "Debug,PartialEq"
)]
pub struct CreatePullRequestMigration;

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "src/graphql/schema.docs.graphql",
    query_path = "src/graphql/GetRepositoryQuery.graphql",
    response_derives = "Debug,PartialEq"
)]
pub struct GetRepositoryQuery;

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "src/graphql/schema.docs.graphql",
    query_path = "src/graphql/UpdatePullRequest.graphql",
    response_derives = "Debug,PartialEq"
)]
pub struct UpdatePullRequestMutation;

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
        Self {
            owner: owner.into(),
            repo: repo.into(),
        }
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
    #[error("GraphQL responded with errors: {error}")]
    GraphQlError { error: String },
    #[error("Repository {owner}/{repo} does not exist")]
    NoSuchRepository { owner: String, repo: String },
    #[error("Pull Request {owner}/{repo}/{number} does not exist")]
    NoSuchPullRequest { owner: String, repo: String, number: i64 },
    #[error("Repository {owner}/{repo} has no default branch")]
    NoDefaultBranch { owner: String, repo: String },
    #[error("Unable to create Pull Request")]
    UnableToCreatePullRequest,
    #[error(transparent)]
    NetworkError(#[from] anyhow::Error),
}

pub async fn post_graphql<Q: GraphQLQuery>(
    client: &reqwest::Client,
    variables: Q::Variables,
) -> Result<graphql_client::Response<Q::ResponseData>, reqwest::Error> {
    let body = Q::build_query(variables);
    debug!("GitHub Body: {:?}", serde_json::to_string(&body));
    let reqwest_response = client.post("https://api.github.com/graphql").json(&body).send().await?;

    Ok(reqwest_response.json().await?)
}

pub enum PullStatus {
    ChecksFailed,
    NeedsApproval,
    Mergeable,
    Merged,
}

pub async fn fetch_pull_status(github_token: &str, pull: &CreatedPullRequest) -> AnyResult<PullStatus> {
    let client = Client::builder()
        .user_agent(format!("clu/{}", env!("CARGO_PKG_VERSION")))
        .default_headers(
            std::iter::once((
                reqwest::header::AUTHORIZATION,
                reqwest::header::HeaderValue::from_str(&format!("Bearer {}", github_token))
                    .unwrap(),
            ))
            .collect(),
        )
        .build()?;

    let gh_pull = fetch_pr_details(&client, pull.owner.clone(), pull.repo.clone(), pull.pr_number).await?;

    if gh_pull.merged {
        return Ok(PullStatus::Merged);
    }

    if gh_pull.mergeable == get_pull_request_status_query::MergeableState::MERGEABLE {
        return Ok(PullStatus::Mergeable);
    }

    let gh_nodes = gh_pull.commits.nodes.unwrap_or_default();
    let gh_commit = match &gh_nodes.first() {
        Some(commit) => {
            match commit {
                Some(commit) => commit,
                None => { return Ok(PullStatus::ChecksFailed) } 
            }
        },
        None => { return Ok(PullStatus::ChecksFailed) } 
    };

    match &gh_commit.commit.status_check_rollup {
        Some(check) => {
            match check.state {
                get_pull_request_status_query::StatusState::SUCCESS | get_pull_request_status_query::StatusState::PENDING => Ok(PullStatus::Mergeable),
                _ => Ok(PullStatus::ChecksFailed)
            }
        },
        None => { return Ok(PullStatus::ChecksFailed) } 
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
    assert_eq!(
        GitHubRepo::new("ethankhall", "clu"),
        extract_github_info("https://github.com/ethankhall/clu").unwrap()
    );
    assert_eq!(
        GitHubRepo::new("ethankhall", "clu"),
        extract_github_info("https://github.com/ethankhall/clu.git").unwrap()
    );
    assert_eq!(
        GitHubRepo::new("ethankhall", "clu"),
        extract_github_info("git@github.com:ethankhall/clu.git").unwrap()
    );
}

pub async fn create_pull_request(
    github_token: &str,
    create_pr: CreatePullRequest<'_>,
) -> Result<i64, anyhow::Error> {

    let client = Client::builder()
        .user_agent(format!("clu/{}", env!("CARGO_PKG_VERSION")))
        .default_headers(
            std::iter::once((
                reqwest::header::AUTHORIZATION,
                reqwest::header::HeaderValue::from_str(&format!("Bearer {}", github_token))
                    .unwrap(),
            ))
            .collect(),
        )
        .build()?;

    let repo_details = fetch_repo_details(&client, create_pr.repo.owner.clone(), create_pr.repo.repo.clone()).await?;

    let variables = create_pull_request_migration::Variables {
        repository_id: repo_details.id,
        base_ref: repo_details.target_branch,
        head_ref: format!("{}{}", repo_details.prefix, create_pr.branch),
        body: create_pr.body.to_owned(),
        title: create_pr.title.to_owned(),
    };

    let created_pr = post_graphql::<CreatePullRequestMigration>(&client, variables).await?;

    debug!("GitHub Response: {:?}", created_pr);

    let response_data: create_pull_request_migration::ResponseData = match created_pr.data {
        Some(data) => data,
        None => bail!(GitHubError::GraphQlError{error: format!("{:?}", created_pr.errors)})
    };

    let pr = match response_data.create_pull_request.map(|it| it.pull_request).flatten() {
        Some(pr) => pr,
        None => bail!(GitHubError::UnableToCreatePullRequest)
    };

    info!("Create PR at {}", pr.permalink);

    Ok(pr.number)
}

async fn fetch_pr_details(client: &reqwest::Client, owner: String, repo: String, pr_number: i64) -> AnyResult<get_pull_request_status_query::GetPullRequestStatusQueryRepositoryPullRequest> {
    let variables = get_pull_request_status_query::Variables {
        owner: owner.clone(),
        repo: repo.clone(),
        number: pr_number
    };

    info!("Getting repo details for {}/{}", &owner, &repo);

    let pr_status = post_graphql::<GetPullRequestStatusQuery>(&client, variables).await?;

    debug!("GitHub Response: {:?}", pr_status);

    let response_data: get_pull_request_status_query::ResponseData = match pr_status.data {
        Some(data) => data,
        None => bail!(GitHubError::GraphQlError{error: format!("{:?}", pr_status.errors)})
    };

    let gh_repository = match response_data.repository {
        Some(r) => r,
        None => bail!(GitHubError::NoSuchRepository { owner: owner.clone(), repo: repo.clone() })
    };

    match gh_repository.pull_request {
        Some(r) => Ok(r),
        None => bail!(GitHubError::NoSuchPullRequest { owner: owner.clone(), repo: repo.clone(), number: pr_number })
    }
}

struct GithubApiRepo {
    id: String,
    target_branch: String,
    prefix: String,
}

async fn fetch_repo_details(client: &reqwest::Client, owner: String, repo: String,) -> AnyResult<GithubApiRepo> {
    let variables = get_repository_query::Variables {
        owner: owner.clone(),
        repo: repo.clone(),
    };

    let pr_status = post_graphql::<GetRepositoryQuery>(&client, variables).await?;

    debug!("GitHub Response: {:?}", pr_status);

    let response_data: get_repository_query::ResponseData = match pr_status.data {
        Some(data) => data,
        None => bail!(GitHubError::GraphQlError{error: format!("{:?}", pr_status.errors)})
    };

    let gh_repository = match response_data.repository {
        Some(r) => r,
        None => bail!(GitHubError::NoSuchRepository { owner: owner.clone(), repo: repo.clone() })
    };

    let repo_id = gh_repository.id;
    debug!("Repo ID: {}", repo_id);

    let default_branch = match gh_repository.default_branch_ref {
        Some(r) => r,
        None => bail!(GitHubError::NoDefaultBranch { owner: owner.clone(), repo: repo.clone() } )
    };

    let target_branch_name = format!("{}{}", default_branch.prefix, default_branch.name);
    debug!("Target Branch: {}", target_branch_name);

    Ok(GithubApiRepo {
      id: repo_id,
      target_branch: target_branch_name,
      prefix:   default_branch.prefix
    })
}

pub async fn update_pull_request(
    github_token: &str,
    pr_number: i64,
    create_pr: CreatePullRequest<'_>,
) -> AnyResult<i64> {

    let client = Client::builder()
        .user_agent(format!("clu/{}", env!("CARGO_PKG_VERSION")))
        .default_headers(
            std::iter::once((
                reqwest::header::AUTHORIZATION,
                reqwest::header::HeaderValue::from_str(&format!("Bearer {}", github_token))
                    .unwrap(),
            ))
            .collect(),
        )
        .build()?;

    let gh_pull = fetch_pr_details(&client, create_pr.repo.owner.clone(), create_pr.repo.repo.clone(), pr_number).await?;
    let pull_id = gh_pull.id;
    let is_open = gh_pull.state == get_pull_request_status_query::PullRequestState::OPEN;

    if !is_open {
        info!("PR ({}) was not open, making a new one", gh_pull.permalink);
        return create_pull_request(github_token, create_pr).await; 
    }

    let variables = update_pull_request_mutation::Variables {
        pull_request_id: pull_id,
        body: create_pr.body.to_owned(),
        title:create_pr.title.to_owned(),
    };

    info!("Updating PR for {}", &create_pr.repo);

    let updated_pr = post_graphql::<UpdatePullRequestMutation>(&client, variables).await?;

    debug!("GitHub Response: {:?}", updated_pr);

    let response_data: update_pull_request_mutation::ResponseData = match updated_pr.data {
        Some(data) => data,
        None => bail!(GitHubError::GraphQlError{error: format!("{:?}", updated_pr.errors)})
    };

    match response_data.update_pull_request.map(|it| it.pull_request).flatten().map(|it| it.permalink) {
        Some(link) => {
            info!("Updated PR {}", link)
        },
        None => bail!(GitHubError::UnableToCreatePullRequest)
    };

    Ok(pr_number)
}
