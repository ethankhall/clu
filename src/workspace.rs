use async_process::Command;
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;
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
    pub root_dir: PathBuf,
    pub working_dir: PathBuf,
}

impl Workspace {
    pub fn new(workspace_dir: &PathBuf) -> Result<Self, std::io::Error> {
        let stdout = File::create(workspace_dir.join("stdout.log"))?;
        let stderr = File::create(workspace_dir.join("stderr.log"))?;

        Ok(Workspace {
            stdout,
            stderr,
            root_dir: workspace_dir.clone(),
            working_dir: workspace_dir.clone(),
        })
    }

    pub async fn run_command(&mut self, args: &str) -> Result<Output, CommandError> {
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
