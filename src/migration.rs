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
    #[error(transparent)]
    CommandError(#[from] crate::workspace::CommandError),
}

#[instrument(name = "migrate", skip(task), fields(name = %task.pretty_name))]
pub async fn run_migration(task: &MigrationTask) -> AnyResult<PullRequest> {
    let work_dir = task.work_dir.canonicalize()?;

    info!("Processing {} in {:?}", task.pretty_name, work_dir);
    let github_repo = crate::github::extract_github_info(&task.repo)?;

    let mut workspace = Workspace::new(&work_dir)?;

    checkout_repo(&task, &mut workspace).await?;
    run_preflight_check(&task, &mut workspace).await?;
    run_migration_script(&task, &mut workspace).await?;
    prepair_pr(&github_repo, &task).await
}

async fn prepair_pr(github_repo: &GitHubRepo, task: &MigrationTask) -> AnyResult<PullRequest> {
    let definition = &task.definition;

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

    workspace.run_command_successfully(&"git push").await
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
