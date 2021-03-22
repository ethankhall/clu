use anyhow::{bail, Result as AnyResult};
use git2::Repository;
use std::env::current_dir;
use std::path::PathBuf;
use thiserror::Error;
use tracing::{info, instrument};

use crate::github::{create_pull_request, CreatePullRequest, GitHubRepo};
use crate::models::{MigrationTask, PullRequest};
use crate::workspace::Workspace;

#[derive(Error, Debug)]
pub enum MigrationError {
    #[error("The working directory has uncommitted changes. Files: {files}")]
    WorkingDirNotClean { files: String },
    #[error("Unable to checkout {repo}. Got error: {source:?}")]
    UnableToCheckoutRepo {
        repo: String,
        #[source]
        source: anyhow::Error,
    },
    #[error("Pre-flight check failed.")]
    PreFlightCheckFailed,
    #[error("Migration ran unsuccessfully.")]
    MigrationFailed {
        #[source]
        source: anyhow::Error,
    },
    #[error("Unable to creat pull request.")]
    UnableToCreatePullRequest {
        #[source]
        source: anyhow::Error,
    },
    #[error("Unable to parse Git Repo.")]
    InvalidGitRepo {
        #[source]
        source: crate::github::GitHubError,
    },
    #[error(transparent)]
    IoError(#[from] std::io::Error),
}

#[instrument(name = "migrate", skip(task), fields(name = %task.pretty_name))]
pub async fn run_migration(task: &MigrationTask) -> Result<PullRequest, MigrationError> {
    let work_dir = task.work_dir.canonicalize()?;

    info!("Processing {} in {:?}", task.pretty_name, work_dir);
    let github_repo = match crate::github::extract_github_info(&task.repo) {
        Ok(repo) => repo,
        Err(e) => return Err(MigrationError::InvalidGitRepo { source: e }),
    };

    let mut workspace = Workspace::new(&work_dir)?;

    if let Err(e) = checkout_repo(&task, &mut workspace).await {
        return Err(MigrationError::UnableToCheckoutRepo {
            repo: task.repo.clone(),
            source: e,
        });
    };

    if run_preflight_check(&task, &mut workspace).await.is_err() {
        return Err(MigrationError::PreFlightCheckFailed);
    };

    if let Err(e) = run_migration_script(&task, &mut workspace).await {
        return Err(MigrationError::MigrationFailed { source: e });
    }

    match prepair_pr(&github_repo, &task, &mut workspace).await {
        Err(e) => Err(MigrationError::UnableToCreatePullRequest { source: e }),
        Ok(pr) => Ok(pr),
    }
}

async fn prepair_pr(
    github_repo: &GitHubRepo,
    task: &MigrationTask,
    workspace: &mut Workspace,
) -> AnyResult<PullRequest> {
    let definition = &task.definition;

    workspace.run_command_successfully(&"git push").await?;

    let pr_number = create_pull_request(
        &task.github_token,
        CreatePullRequest {
            repo: &github_repo,
            branch: &definition.checkout.branch_name,
            title: &definition.pr.title,
            body: &definition.pr.description,
        },
    )
    .await?;

    Ok(PullRequest {
        owner: github_repo.owner.clone(),
        repo: github_repo.repo.clone(),
        pr_number,
    })
}

async fn run_migration_script(task: &MigrationTask, workspace: &mut Workspace) -> AnyResult<()> {
    let definition = &task.definition;

    for step in &definition.steps {
        info!("Running {} for {}", step.name, task.pretty_name);
        workspace
            .run_command_successfully(&make_script_absolute(&step.migration_script)?)
            .await?;
        info!("Migration script finished successfully");
    }

    let git_repo = workspace.root_dir.join("repo");

    let repo = Repository::open(git_repo)?;
    let status = repo.statuses(None)?;
    if !status.is_empty() {
        let files: Vec<String> = status
            .iter()
            .map(|x| x.path().unwrap().to_owned())
            .collect();
        bail!(MigrationError::WorkingDirNotClean {
            files: files.join(", ")
        })
    }

    Ok(())
}

async fn run_preflight_check(task: &MigrationTask, workspace: &mut Workspace) -> AnyResult<()> {
    let definition = &task.definition;
    info!("Running pre-flight check for {}", task.pretty_name);
    workspace
        .run_command_successfully(&make_script_absolute(&definition.checkout.pre_flight)?)
        .await?;
    info!("Preflight check was successful");

    Ok(())
}

fn make_script_absolute(path: &str) -> AnyResult<String> {
    let mut preflight_check = PathBuf::from(&path);
    if !preflight_check.is_absolute() {
        preflight_check = current_dir()?.join(preflight_check);
    }

    let preflight_check = preflight_check.to_str().unwrap();
    Ok(preflight_check.to_owned())
}

async fn checkout_repo(task: &MigrationTask, workspace: &mut Workspace) -> AnyResult<()> {
    let git_repo = workspace.root_dir.join("repo");
    let definition = &task.definition;

    info!(
        "Cloning {} into {}",
        task.pretty_name,
        git_repo.to_str().unwrap()
    );

    workspace
        .run_command_successfully(&format!(
            "git clone {} {}",
            &task.repo,
            git_repo.to_str().unwrap()
        ))
        .await?;
    workspace.set_working_dir("repo");
    workspace
        .run_command_successfully(&format!(
            "git checkout -b {}",
            &definition.checkout.branch_name
        ))
        .await?;

    Ok(())
}
