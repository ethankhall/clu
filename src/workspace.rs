use anyhow::{bail, Result as AnyResult};
use async_process::Command;
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;
use std::process::Output;
use thiserror::Error;
use tracing::debug;

#[derive(Error, Debug)]
#[error("{command} exited with {code}. You can check {working_dir} for the output files")]
pub struct CommandError {
    command: String,
    working_dir: String,
    code: i32,
}

pub struct Workspace {
    stdout: File,
    stderr: File,
    pub root_dir: PathBuf,
    pub working_dir: PathBuf,
}

impl Workspace {
    pub fn new(workspace_dir: &PathBuf) -> AnyResult<Self> {
        let stdout = File::create(workspace_dir.join("stdout.log"))?;
        let stderr = File::create(workspace_dir.join("stderr.log"))?;

        Ok(Workspace {
            stdout,
            stderr,
            root_dir: workspace_dir.clone(),
            working_dir: workspace_dir.clone(),
        })
    }

    pub async fn run_command(&mut self, args: &str) -> AnyResult<Output> {
        debug!("Running {}", args);
        
        let notification = format!(">> Running {}\n", args);
        self.stdout.write_all(notification.as_bytes())?;
        self.stderr.write_all(notification.as_bytes())?;

        let output = Command::new("/bin/sh")
            .arg("-c")
            .arg(args)
            .current_dir(&self.working_dir)
            .output()
            .await?;

        self.stdout.write_all(&output.stdout)?;
        self.stderr.write_all(&output.stderr)?;

        Ok(output)
    }

    pub async fn run_command_successfully(&mut self, args: &str) -> AnyResult<()> {
        let status = self.run_command(args).await?.status;
        if !status.success() {
            bail!(CommandError {
                code: status.code().unwrap(),
                command: args.to_owned(),
                working_dir: self.working_dir.to_str().unwrap().to_owned()
            });
        }

        Ok(())
    }

    pub fn set_working_dir(&mut self, path: &str) {
        self.working_dir = self.root_dir.join(path);
    }
}
