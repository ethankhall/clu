use async_process::Command;
use std::collections::BTreeMap;
use std::fs::{create_dir_all, remove_dir_all, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Output;
use thiserror::Error;
use tracing::debug;

#[derive(Error, Debug)]
pub enum CommandError {
    #[error("{command} exited with {code}. You can check {working_dir} for the output files")]
    NonZeroExit {
        command: String,
        working_dir: String,
        code: i32,
    },
    #[error(transparent)]
    IoError(#[from] std::io::Error),
}

pub struct Workspace {
    stdout: File,
    stderr: File,
    env_vars: BTreeMap<String, String>,
    pub root_dir: PathBuf,
    pub working_dir: PathBuf,
    pub workspace_name: String,
}

impl Workspace {
    pub fn new_clean_workspace<S: Into<String>>(
        workspace_name: S,
        workspace_dir: &Path,
    ) -> Result<Self, std::io::Error> {
        let workspace_name = workspace_name.into();
        debug!("Processing {:?}", workspace_name);
        if workspace_dir.exists() {
            remove_dir_all(&workspace_dir)?
        }
        create_dir_all(&workspace_dir)?;

        Self::new(workspace_name, workspace_dir)
    }

    pub fn new<S: Into<String>>(
        workspace_name: S,
        workspace_dir: &Path,
    ) -> Result<Self, std::io::Error> {
        let stdout = File::create(workspace_dir.join("stdout.log"))?;
        let stderr = File::create(workspace_dir.join("stderr.log"))?;

        Ok(Workspace {
            workspace_name: workspace_name.into(),
            stdout,
            stderr,
            env_vars: BTreeMap::new(),
            root_dir: workspace_dir.to_path_buf(),
            working_dir: workspace_dir.to_path_buf(),
        })
    }

    pub fn set_env_vars(&mut self, envs: &mut BTreeMap<String, String>) {
        self.env_vars.clear();
        self.env_vars.append(envs);
    }

    pub async fn run_command(&mut self, args: &str) -> Result<Output, CommandError> {
        debug!("Running {}", args);

        let notification = format!(">> Running {}\n", args);
        self.stdout.write_all(notification.as_bytes())?;
        self.stderr.write_all(notification.as_bytes())?;

        let envs: Vec<(String, String)> = self
            .env_vars
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        let output = Command::new("/bin/sh")
            .arg("-c")
            .arg(args)
            .envs(envs)
            .current_dir(&self.working_dir)
            .output()
            .await?;

        self.stdout.write_all(&output.stdout)?;
        self.stderr.write_all(&output.stderr)?;

        Ok(output)
    }

    pub async fn run_command_successfully(&mut self, args: &str) -> Result<(), CommandError> {
        let status = self.run_command(args).await?.status;
        if !status.success() {
            Err(CommandError::NonZeroExit {
                code: status.code().unwrap(),
                command: args.to_owned(),
                working_dir: self.working_dir.to_str().unwrap().to_owned(),
            })
        } else {
            Ok(())
        }
    }

    pub fn set_working_dir(&mut self, path: &str) {
        self.working_dir = self.root_dir.join(path);
    }
}
