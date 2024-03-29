use async_trait::async_trait;
use std::env::current_dir;
use std::path::PathBuf;

mod git;
mod github;
mod script_exec;

use crate::migration::MigrationError;
use crate::workspace::Workspace;

use git::RepoCheck;
pub use git::{CloneRepoStep, PushRepoStep};
pub use github::UpdateGithubStep;
pub use script_exec::{FollowUpStep, MigrationScriptStep, PreFlightCheckStep};

#[async_trait]
pub trait MigrationStep<Output> {
    async fn execute_step(&self, workspace: &mut Workspace) -> MigrationStepResult<Output>;
}

#[derive(Debug)]
pub struct MigrationStepResult<Output> {
    pub name: String,
    pub terminal: bool,
    pub result: Result<Output, MigrationError>,
    pub did_execute: bool,
}

impl<Output> MigrationStepResult<Output> {
    pub fn success_with_result<S: Into<String>>(name: S, result: Output) -> Self {
        Self {
            name: name.into(),
            terminal: false,
            result: Ok(result),
            did_execute: true,
        }
    }

    pub fn failure<S: Into<String>>(name: S, error: MigrationError) -> Self {
        Self {
            name: name.into(),
            terminal: true,
            result: Err(error),
            did_execute: true,
        }
    }
}

impl MigrationStepResult<()> {
    pub fn success<S: Into<String>>(name: S) -> Self {
        Self::success_with_result(name, ())
    }

    pub fn abort<S: Into<String>>(name: S) -> Self {
        Self {
            name: name.into(),
            terminal: true,
            result: Ok(()),
            did_execute: true,
        }
    }
}

fn make_script_absolute(path: &str) -> String {
    let mut preflight_check = PathBuf::from(&path);
    if !preflight_check.is_absolute() {
        preflight_check = current_dir()
            .expect("Unable to get current dir")
            .join(preflight_check);
    }

    let preflight_check = preflight_check.to_str().unwrap();
    preflight_check.to_owned()
}
