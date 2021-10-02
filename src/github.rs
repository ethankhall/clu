use anyhow::{bail, Result as AnyResult};
use graphql_client::GraphQLQuery;
use regex::Regex;
use reqwest::Client;
use std::fmt;
use thiserror::Error;
use tracing::{debug, info};

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

pub struct PullRequestDescription<'a> {
    pub branch: &'a str,
    pub title: &'a str,
    pub body: &'a str,
}

#[derive(Debug)]
pub struct PullRequestOutput {
    pub number: i64,
    pub permalink: String,
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
    NoSuchPullRequest {
        owner: String,
        repo: String,
        number: i64,
    },
    #[error("Repository {owner}/{repo} has no default branch")]
    NoDefaultBranch { owner: String, repo: String },
    #[error("Unable to create Pull Request")]
    UnableToCreatePullRequest,
    #[error(transparent)]
    NetworkError(#[from] anyhow::Error),
}

#[derive(Debug)]
pub struct GithubApiClient {
    client: Client,
}

impl GithubApiClient {
    pub fn new(github_token: &str) -> Result<Self, anyhow::Error> {
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

        Ok(Self { client })
    }

    pub async fn fetch_pull_state(
        &self,
        repo: &GitHubRepo,
        pr_number: i64,
    ) -> AnyResult<PullState> {
        let gh_pull = fetch_pr_details(
            &self.client,
            repo.owner.clone(),
            repo.repo.clone(),
            pr_number,
        )
        .await?;

        if gh_pull.merged {
            return Ok(PullState {
                permalink: gh_pull.permalink,
                status: PullStatus::Merged,
            });
        }

        if gh_pull.mergeable == get_pull_request_status_query::MergeableState::MERGEABLE {
            return Ok(PullState {
                permalink: gh_pull.permalink,
                status: PullStatus::Mergeable,
            });
        }

        let gh_nodes = gh_pull.commits.nodes.unwrap_or_default();
        let gh_commit = match &gh_nodes.first() {
            Some(Some(commit)) => commit,
            _ => {
                return Ok(PullState {
                    permalink: gh_pull.permalink,
                    status: PullStatus::ChecksFailed,
                })
            }
        };

        match &gh_commit.commit.status_check_rollup {
            Some(check) => match check.state {
                get_pull_request_status_query::StatusState::SUCCESS
                | get_pull_request_status_query::StatusState::PENDING => Ok(PullState {
                    permalink: gh_pull.permalink,
                    status: PullStatus::Mergeable,
                }),
                _ => Ok(PullState {
                    permalink: gh_pull.permalink,
                    status: PullStatus::ChecksFailed,
                }),
            },
            None => Ok(PullState {
                permalink: gh_pull.permalink,
                status: PullStatus::ChecksFailed,
            }),
        }
    }

    pub async fn sync_pull_request(
        &self,
        repo: &GitHubRepo,
        pr_description: PullRequestDescription<'_>,
        pr_number: Option<i64>,
    ) -> AnyResult<PullRequestOutput> {
        let update_pr = match pr_number {
            Some(num) => self.is_pr_open(repo, num).await?,
            None => false,
        };

        if update_pr {
            self.update_pull_request(repo, pr_description, pr_number.unwrap())
                .await
        } else {
            self.create_pull_request(repo, pr_description).await
        }
    }

    async fn is_pr_open(&self, repo: &GitHubRepo, pr_number: i64) -> AnyResult<bool> {
        let gh_pull = fetch_pr_details(
            &self.client,
            repo.owner.clone(),
            repo.repo.clone(),
            pr_number,
        )
        .await?;
        let is_open = gh_pull.state == get_pull_request_status_query::PullRequestState::OPEN;

        Ok(is_open)
    }

    async fn update_pull_request(
        &self,
        repo: &GitHubRepo,
        pr_description: PullRequestDescription<'_>,
        pr_number: i64,
    ) -> AnyResult<PullRequestOutput> {
        let gh_pull = fetch_pr_details(
            &self.client,
            repo.owner.clone(),
            repo.repo.clone(),
            pr_number,
        )
        .await?;
        let pull_id = gh_pull.id;

        let variables = update_pull_request_mutation::Variables {
            pull_request_id: pull_id,
            body: pr_description.body.to_owned(),
            title: pr_description.title.to_owned(),
        };

        info!("Updating PR for {}", &repo);

        let updated_pr = post_graphql::<UpdatePullRequestMutation>(&self.client, variables).await?;

        debug!("GitHub Response: {:?}", updated_pr);

        let response_data: update_pull_request_mutation::ResponseData = match updated_pr.data {
            Some(data) => data,
            None => bail!(GitHubError::GraphQlError {
                error: format!("{:?}", updated_pr.errors)
            }),
        };

        let pr = match response_data
            .update_pull_request
            .map(|it| it.pull_request)
            .flatten()
        {
            Some(pr) => pr,
            None => bail!(GitHubError::UnableToCreatePullRequest),
        };

        info!("Updated PR {}", pr.permalink);

        Ok(PullRequestOutput {
            number: pr.number,
            permalink: pr.permalink,
        })
    }

    async fn create_pull_request(
        &self,
        repo: &GitHubRepo,
        pr_description: PullRequestDescription<'_>,
    ) -> Result<PullRequestOutput, anyhow::Error> {
        let repo_details =
            fetch_repo_details(&self.client, repo.owner.clone(), repo.repo.clone()).await?;

        let variables = create_pull_request_migration::Variables {
            repository_id: repo_details.id,
            base_ref: repo_details.target_branch,
            head_ref: format!("{}{}", repo_details.prefix, pr_description.branch),
            body: pr_description.body.to_owned(),
            title: pr_description.title.to_owned(),
        };

        let created_pr =
            post_graphql::<CreatePullRequestMigration>(&self.client, variables).await?;

        debug!("GitHub Response: {:?}", created_pr);

        let response_data: create_pull_request_migration::ResponseData = match created_pr.data {
            Some(data) => data,
            None => bail!(GitHubError::GraphQlError {
                error: format!("{:?}", created_pr.errors)
            }),
        };

        let pr = match response_data
            .create_pull_request
            .map(|it| it.pull_request)
            .flatten()
        {
            Some(pr) => pr,
            None => bail!(GitHubError::UnableToCreatePullRequest),
        };

        info!("Create PR at {}", pr.permalink);

        Ok(PullRequestOutput {
            number: pr.number,
            permalink: pr.permalink,
        })
    }
}

pub async fn post_graphql<Q: GraphQLQuery>(
    client: &reqwest::Client,
    variables: Q::Variables,
) -> Result<graphql_client::Response<Q::ResponseData>, reqwest::Error> {
    let body = Q::build_query(variables);
    debug!("GitHub Body: {:?}", serde_json::to_string(&body));
    let reqwest_response = client
        .post("https://api.github.com/graphql")
        .json(&body)
        .send()
        .await?;

    Ok(reqwest_response.json().await?)
}

pub struct PullState {
    pub status: PullStatus,
    pub permalink: String,
}

pub enum PullStatus {
    ChecksFailed,
    NeedsApproval,
    Mergeable,
    Merged,
}

pub async fn fetch_pull_state(
    github_token: &str,
    repo: &GitHubRepo,
    pr_number: i64,
) -> AnyResult<PullState> {
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

    let gh_pull =
        fetch_pr_details(&client, repo.owner.clone(), repo.repo.clone(), pr_number).await?;

    if gh_pull.merged {
        return Ok(PullState {
            permalink: gh_pull.permalink,
            status: PullStatus::Merged,
        });
    }

    if gh_pull.mergeable == get_pull_request_status_query::MergeableState::MERGEABLE {
        return Ok(PullState {
            permalink: gh_pull.permalink,
            status: PullStatus::Mergeable,
        });
    }

    let gh_nodes = gh_pull.commits.nodes.unwrap_or_default();
    let gh_commit = match &gh_nodes.first() {
        Some(Some(commit)) => commit,
        _ => {
            return Ok(PullState {
                permalink: gh_pull.permalink,
                status: PullStatus::ChecksFailed,
            })
        }
    };

    match &gh_commit.commit.status_check_rollup {
        Some(check) => match check.state {
            get_pull_request_status_query::StatusState::SUCCESS
            | get_pull_request_status_query::StatusState::PENDING => Ok(PullState {
                permalink: gh_pull.permalink,
                status: PullStatus::Mergeable,
            }),
            _ => Ok(PullState {
                permalink: gh_pull.permalink,
                status: PullStatus::ChecksFailed,
            }),
        },
        None => Ok(PullState {
            permalink: gh_pull.permalink,
            status: PullStatus::ChecksFailed,
        }),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHubRepo {
    pub owner: String,
    pub repo: String,
    pub clone_url: String,
}

impl GitHubRepo {
    fn new<G: Into<String>>(owner: G, repo: G, clone_url: G) -> Self {
        Self {
            owner: owner.into(),
            repo: repo.into(),
            clone_url: clone_url.into(),
        }
    }
}

impl fmt::Display for GitHubRepo {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}/{}", self.owner, self.repo)
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

            Ok(GitHubRepo::new(owner, repo, url))
        }
        None => Err(GitHubError::UnableToDetermineRepo {
            path: url.to_owned(),
        }),
    }
}

#[test]
fn validate_extract_github_info() {
    assert_eq!(
        "ethankhall",
        extract_github_info("https://github.com/ethankhall/clu")
            .unwrap()
            .owner
    );
    assert_eq!(
        "clu",
        extract_github_info("https://github.com/ethankhall/clu")
            .unwrap()
            .repo
    );
    assert_eq!(
        "ethankhall",
        extract_github_info("https://github.com/ethankhall/clu.git")
            .unwrap()
            .owner
    );
    assert_eq!(
        "clu",
        extract_github_info("https://github.com/ethankhall/clu.git")
            .unwrap()
            .repo
    );
    assert_eq!(
        "ethankhall",
        extract_github_info("git@github.com:ethankhall/clu.git")
            .unwrap()
            .owner
    );
    assert_eq!(
        "clu",
        extract_github_info("git@github.com:ethankhall/clu.git")
            .unwrap()
            .repo
    );
}

async fn fetch_pr_details(
    client: &reqwest::Client,
    owner: String,
    repo: String,
    pr_number: i64,
) -> AnyResult<get_pull_request_status_query::GetPullRequestStatusQueryRepositoryPullRequest> {
    let variables = get_pull_request_status_query::Variables {
        owner: owner.clone(),
        repo: repo.clone(),
        number: pr_number,
    };

    info!("Getting repo details for {}/{}", &owner, &repo);

    let pr_status = post_graphql::<GetPullRequestStatusQuery>(client, variables).await?;

    debug!("GitHub Response: {:?}", pr_status);

    let response_data: get_pull_request_status_query::ResponseData = match pr_status.data {
        Some(data) => data,
        None => bail!(GitHubError::GraphQlError {
            error: format!("{:?}", pr_status.errors)
        }),
    };

    let gh_repository = match response_data.repository {
        Some(r) => r,
        None => bail!(GitHubError::NoSuchRepository {
            owner: owner.clone(),
            repo: repo.clone()
        }),
    };

    match gh_repository.pull_request {
        Some(r) => Ok(r),
        None => bail!(GitHubError::NoSuchPullRequest {
            owner: owner.clone(),
            repo: repo.clone(),
            number: pr_number
        }),
    }
}

struct GithubApiRepo {
    id: String,
    target_branch: String,
    prefix: String,
}

async fn fetch_repo_details(
    client: &reqwest::Client,
    owner: String,
    repo: String,
) -> AnyResult<GithubApiRepo> {
    let variables = get_repository_query::Variables {
        owner: owner.clone(),
        repo: repo.clone(),
    };

    let pr_status = post_graphql::<GetRepositoryQuery>(client, variables).await?;

    debug!("GitHub Response: {:?}", pr_status);

    let response_data: get_repository_query::ResponseData = match pr_status.data {
        Some(data) => data,
        None => bail!(GitHubError::GraphQlError {
            error: format!("{:?}", pr_status.errors)
        }),
    };

    let gh_repository = match response_data.repository {
        Some(r) => r,
        None => bail!(GitHubError::NoSuchRepository {
            owner: owner.clone(),
            repo: repo.clone()
        }),
    };

    let repo_id = gh_repository.id;
    debug!("Repo ID: {}", repo_id);

    let default_branch = match gh_repository.default_branch_ref {
        Some(r) => r,
        None => bail!(GitHubError::NoDefaultBranch {
            owner: owner.clone(),
            repo: repo.clone()
        }),
    };

    let target_branch_name = format!("{}{}", default_branch.prefix, default_branch.name);
    debug!("Target Branch: {}", target_branch_name);

    Ok(GithubApiRepo {
        id: repo_id,
        target_branch: target_branch_name,
        prefix: default_branch.prefix,
    })
}
