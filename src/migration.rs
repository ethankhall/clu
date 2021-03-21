use anyhow::{bail, Result as AnyResult};
use git2::Repository;
use thiserror::Error;
use tracing::{info, instrument};
use std::path::{PathBuf};
use std::env::current_dir;

use crate::github::{create_pull_request, GitHubRepo, CreatePullRequest};
use crate::models::{MigrationInput, PullRequest};
use crate::workspace::Workspace;

#[derive(Error, Debug)]
pub enum MigrationError {
    #[error("The working directory has uncommitted changes. Files: {files}")]
    WorkingDirNotClean { files: String },
    #[error(transparent)]
    CommandError(#[from] crate::workspace::CommandError),
}

#[instrument(name = "migrate", skip(migration_input), fields(name = %migration_input.target.pretty_name))]
pub async fn run_migration(migration_input: MigrationInput) -> AnyResult<PullRequest> {
    let target = &migration_input.target;
    let work_dir = migration_input.work_dir.canonicalize()?;

    info!("Processing {} in {:?}", target.pretty_name, work_dir);
    let github_repo = crate::github::extract_github_info(&migration_input.target.repo_path)?;

    let mut workspace = Workspace::new(&work_dir)?;

    checkout_repo(&migration_input, &mut workspace).await?;
    run_preflight_check(&migration_input, &mut workspace).await?;
    run_migration_script(&migration_input, &mut workspace).await?;
    prepair_pr(&github_repo, &migration_input).await
}

async fn prepair_pr(github_repo: &GitHubRepo, migration_input: &MigrationInput) -> AnyResult<PullRequest> {
    let definition = &migration_input.definition;

    let pr_number = create_pull_request(
        &migration_input.github_token,
        CreatePullRequest {
            repo: &github_repo,
            branch: &definition.checkout.branch_name,
            title: &definition.pr.title,
            body: &definition.pr.description,
        },
    ).await?;

    Ok(PullRequest { owner: github_repo.owner.clone(), repo: github_repo.repo.clone(), pr_number })
}

async fn run_migration_script(
    migration_input: &MigrationInput,
    workspace: &mut Workspace,
) -> AnyResult<()> {
    let target = &migration_input.target;
    let definition = &migration_input.definition;

    for step in &definition.steps {
        info!("Running {} for {}", step.name, target.pretty_name);
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

async fn run_preflight_check(
    migration_input: &MigrationInput,
    workspace: &mut Workspace,
) -> AnyResult<()> {
    let target = &migration_input.target;
    let definition = &migration_input.definition;
    info!("Running pre-flight check for {}", target.pretty_name);
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

async fn checkout_repo(
    migration_input: &MigrationInput,
    workspace: &mut Workspace,
) -> AnyResult<()> {
    let git_repo = workspace.root_dir.join("repo");
    let target = &migration_input.target;
    let definition = &migration_input.definition;

    info!(
        "Cloning {} into {}",
        target.pretty_name,
        git_repo.to_str().unwrap()
    );

    workspace
        .run_command_successfully(&format!(
            "git clone {} {}",
            &target.repo_path,
            git_repo.to_str().unwrap()
        ))
        .await?;
    workspace.set_working_dir("repo");
    workspace
        .run_command_successfully(&format!("git checkout -b {}", &definition.checkout.branch_name))
        .await?;

    Ok(())
}
