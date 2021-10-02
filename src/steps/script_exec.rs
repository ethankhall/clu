use anyhow::Result as AnyResult;
use async_trait::async_trait;
use tracing::{info, instrument, warn};

use std::env::current_dir;
use std::path::PathBuf;

use super::{MigrationStep, MigrationStepResult, RepoCheck};
use crate::migration::{MigrationError, MigrationTask};
use crate::models::MigrationStepDefinition;
use crate::workspace::{CommandError, Workspace};

pub struct PreFlightCheckStep<'a> {
    command: &'a str,
}

#[async_trait]
impl<'a> MigrationStep<()> for PreFlightCheckStep<'a> {
    #[instrument(name = "pre-flight", skip(self, workspace), fields(workspace_name = %workspace.workspace_name, command = %self.command))]
    async fn execute_step(&self, workspace: &mut Workspace) -> MigrationStepResult<()> {
        match self.run_preflight(workspace).await {
            Ok(_) => MigrationStepResult::success("pre-flight"),
            Err(_) => MigrationStepResult::abort("pre-flight"),
        }
    }
}

impl<'a> PreFlightCheckStep<'a> {
    async fn run_preflight(&self, workspace: &mut Workspace) -> AnyResult<()> {
        info!("Running pre-flight check for {}", workspace.workspace_name);
        if let Err(e) = workspace
            .run_command_successfully(&make_script_absolute(self.command))
            .await
        {
            info!("Preflight check determined the migration is complete.");
            anyhow::bail!(e);
        }
        info!("Preflight check determined the migration should be run.");

        Ok(())
    }

    fn new(command: &'a str) -> Self {
        Self { command }
    }
}

impl<'a> From<&'a MigrationTask<'_>> for PreFlightCheckStep<'a> {
    fn from(task: &'a MigrationTask) -> Self {
        Self::new(&task.definition.checkout.pre_flight)
    }
}

pub struct MigrationScriptStep<'a> {
    step_name: &'a str,
    command: &'a str,
}

#[async_trait]
impl<'a> MigrationStep<()> for MigrationScriptStep<'a> {
    #[instrument(name = "migration", skip(self, workspace), fields(workspace_name = %workspace.workspace_name, step_name = %self.step_name, command = %self.command))]
    async fn execute_step(&self, workspace: &mut Workspace) -> MigrationStepResult<()> {
        info!("Running migration script");
        if let Err(e) = workspace
            .run_command_successfully(&make_script_absolute(self.command))
            .await
        {
            match e {
                CommandError::NonZeroExit {
                    code,
                    command: _,
                    working_dir: _,
                } => {
                    warn!("Migration script exited with code {}", code);
                }
                CommandError::IoError(err) => {
                    warn!("Migration script encountered error: {}", err);
                }
            };

            return MigrationStepResult::failure(
                "migration-step:exec",
                MigrationError::MigrationStepErrored {
                    step_name: self.step_name.to_owned(),
                },
            );
        }

        if let Err(e) = RepoCheck::check_for_untracked_files(self.step_name, workspace).await {
            return MigrationStepResult::failure("migration-step:untracked_files", e);
        }

        info!("Migration script finished successfully");

        MigrationStepResult::success("migration-step")
    }
}

impl<'a> MigrationScriptStep<'a> {
    fn new(step_name: &'a str, command: &'a str) -> Self {
        Self { step_name, command }
    }
}

impl<'a> From<&'a MigrationStepDefinition> for MigrationScriptStep<'a> {
    fn from(step_def: &'a MigrationStepDefinition) -> Self {
        Self::new(&step_def.name, &step_def.migration_script)
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
