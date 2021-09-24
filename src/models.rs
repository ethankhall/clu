use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct MigrationDefinition {
    pub checkout: RepoCheckout,

    pub pr: PrCreationDetails,

    pub steps: Vec<MigrationStep>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct RepoCheckout {
    /// The name of the branch that should be pushed up to GitHub. This should be
    /// something semi unique, and would recommend to include the name of the migration
    /// and date in it.
    pub branch_name: String,

    /// Path to a script that will be executed on the repo. If the script
    /// returns an exit-code 0, then the migration will continue. Any other
    /// value will cause the migration to be skipped for this repo.
    pub pre_flight: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct MigrationStep {
    /// Name of the migration step, only used for reporting.
    pub name: String,

    /// The script that will run against the repo, if it exits with an exit code 0
    /// the changes will be added to a branch and then pushed up to GitHub. If
    /// the exit code is not 0, then the migration will not publish the results.
    ///
    /// If there are ANY untracked changes, the migration WILL fail to publish.
    /// The migration script NEEDS to commit the changes they want.
    pub migration_script: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct PrCreationDetails {
    /// The titile of the PR.
    pub title: String,

    /// This message will also show up in the GitHub PR.
    pub description: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct MigrationFile {
    pub targets: BTreeMap<String, TargetDescription>,
    #[serde(flatten)]
    pub definition: MigrationDefinition,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct TargetDescription {
    pub repo: String,
    pub env: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub pull_request: Option<CreatedPullRequest>,
}

impl TargetDescription {
    pub fn new(repo: &str) -> Self {
        Self {
            repo: repo.to_owned(),
            env: None,
            pull_request: None,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CreatedPullRequest {
    pub pr_number: i64,
    #[serde(default)]
    pub url: String,
}
