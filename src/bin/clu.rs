use clap::{AppSettings, ArgGroup, Clap};
use futures::stream::{self, StreamExt};
use std::fs::{create_dir_all, read_to_string, File};
use std::io::Write;
use std::path::PathBuf;
use std::time::SystemTime;

use anyhow::Result as AnyResult;
use tracing::{debug, error, info, warn};

use clu::commands::*;
use clu::github::GithubApiClient;
use clu::migration::{ExecutionOptions, MigrationStatus, MigrationTask};
use clu::models::*;

/// Clu is a migration tool, intended to make cross company migrations easier
///
/// ## Run a Migration
///
/// > clu run-migration --migration-definition migration.toml
///
/// When `clu` is done, it will update `migration.toml` (and save a backup).
///
/// ## Check Status
///
/// Checks the status of the PR's that were created.
///
/// > clu check-status --results migration.toml
#[derive(Clap, Debug)]
#[clap(author, version)]
#[clap(setting = AppSettings::ColoredHelp)]
pub struct Opts {
    #[clap(flatten)]
    pub logging_opts: LoggingOpts,

    #[clap(subcommand)]
    pub sub_command: SubCommand,
}

#[derive(Clap, Debug)]
pub enum SubCommand {
    /// Build a default migration toml file
    Init,
    /// Run a migration, and write the results back to the file.
    RunMigration(RunMigrationArgs),
    /// Check the status of a migration.
    CheckStatus(CheckStatusArgs),
    /// Runs a script against each open PR.
    RunFollowup(RunFollowupArgs),
}

#[derive(Clap, Debug)]
pub struct CheckStatusArgs {
    /// A TOML file that defines the input needed to run a migration. This file will be updated
    /// with the results of the run.
    #[clap(long)]
    pub migration_definition: String,

    /// Token to be used when talking to GitHub
    #[clap(long, env = "GITHUB_TOKEN")]
    pub github_token: String,
}

#[derive(Clap, Debug)]
pub struct RunMigrationArgs {
    /// A TOML file that defines the input needed to run a migration. This file will be updated
    /// with the results of the run.
    #[clap(long)]
    pub migration_definition: String,

    /// Folder where the work will take place
    #[clap(long = "work-directory", default_value("work-dir"))]
    pub work_directory_root: String,

    /// Token to be used when talking to GitHub
    #[clap(long, env = "GITHUB_TOKEN")]
    pub github_token: String,

    #[clap(flatten)]
    pub dry_run_opts: DryRunOpts,
}

#[derive(Clap, Debug)]
#[clap(group = ArgGroup::new("publish-group"))]
pub struct DryRunOpts {
    /// When set, the PR will not be created. The change will still be pushed to the
    /// upstream repo.
    #[clap(long, group = "publish-group")]
    pub skip_pull_request: bool,

    /// When set, the git repo will not be updated. When set, an existing PR will be
    /// updated (if applicable).
    #[clap(long, group = "publish-group")]
    pub skip_push: bool,

    /// The remote will not be updated, the PR will not be updated. Local scripts will run.
    #[clap(long, group = "publish-group")]
    pub dry_run: bool,
}

#[derive(Clap, Debug)]
#[clap(group = ArgGroup::new("logging"))]
pub struct LoggingOpts {
    /// A level of verbosity, and can be used multiple times
    #[clap(short, long, parse(from_occurrences), global(true), group = "logging")]
    pub debug: u64,

    /// Enable warn logging
    #[clap(short, long, global(true), group = "logging")]
    pub warn: bool,

    /// Disable everything but error logging
    #[clap(short, long, global(true), group = "logging")]
    pub error: bool,
}

impl LoggingOpts {
    pub fn to_level(&self) -> tracing::Level {
        use tracing::Level;

        if self.error {
            Level::ERROR
        } else if self.warn {
            Level::WARN
        } else if self.debug == 0 {
            Level::INFO
        } else if self.debug == 1 {
            Level::DEBUG
        } else {
            Level::TRACE
        }
    }
}

#[tokio::main]
async fn main() -> AnyResult<()> {
    dotenv::dotenv().ok();

    let opt = Opts::parse();
    configure_logging(&opt.logging_opts);

    match opt.sub_command {
        SubCommand::Init => run_init().await,
        SubCommand::RunMigration(args) => run_migration(args).await,
        SubCommand::CheckStatus(args) => check_status(args).await,
        SubCommand::RunFollowup(args) => run_followup(args).await,
    }
}

async fn check_status(args: CheckStatusArgs) -> AnyResult<()> {
    use clu::github::PullStatus;

    let mut checks_failed: Vec<String> = Vec::new();
    let mut not_approved: Vec<String> = Vec::new();
    let mut mergeable: Vec<String> = Vec::new();
    let mut merged: Vec<String> = Vec::new();

    let results: MigrationFile = toml::from_str(&read_to_string(args.migration_definition)?)?;
    let github_api = GithubApiClient::new(&args.github_token)?;
    for (_name, target) in results.targets {
        let pull = match target.pull_request {
            Some(pull) => pull,
            _ => continue,
        };

        let github_repo = clu::github::extract_github_info(&target.repo)?;

        let state = github_api
            .fetch_pull_state(&github_repo, pull.pr_number)
            .await?;

        match state.status {
            PullStatus::ChecksFailed => checks_failed.push(format!("- {}", state.permalink)),
            PullStatus::NeedsApproval => not_approved.push(format!("- {}", state.permalink)),
            PullStatus::Mergeable => mergeable.push(format!("- {}", state.permalink)),
            PullStatus::Merged => merged.push(format!("- {}", state.permalink)),
        }
    }

    checks_failed.sort();
    not_approved.sort();
    mergeable.sort();
    merged.sort();

    println!(
        "# Migration Results
## Checks Failed

{}

## Not Approved

{}

## Mergeable

{}

## Merged

{}",
        checks_failed.join("\n"),
        not_approved.join("\n"),
        mergeable.join("\n"),
        merged.join("\n")
    );

    Ok(())
}

async fn run_init() -> AnyResult<()> {
    use std::collections::BTreeMap;
    let mut targets = BTreeMap::new();
    targets.insert(
        "dummy-repo".to_owned(),
        TargetDescription::new("git@github.com:ethankhall/dummy-repo.git"),
    );

    let definition = MigrationDefinition {
        checkout: RepoCheckout {
            branch_name: "ethankhall/foo-example".to_owned(),
            pre_flight: "/usr/bin/true".to_owned(),
        },
        pr: PrCreationDetails {
            title: "Example Title".to_owned(),
            description: "This is a TOML file\n\nSo you can add newlines between the PR's"
                .to_owned(),
        },
        steps: vec![MigrationStepDefinition {
            name: "Example".to_owned(),
            migration_script: "examples/example-migration.sh".to_owned(),
        }],
    };

    let migration_input = MigrationFile {
        targets,
        definition,
    };

    let definition = toml::to_string_pretty(&migration_input)?;

    let mut f = File::create("migration.toml")?;
    f.write_all(definition.as_bytes())
        .expect("Unable to write data");

    Ok(())
}

pub async fn run_migration(args: RunMigrationArgs) -> AnyResult<()> {
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    let mut migration_input: MigrationFile =
        toml::from_str(&read_to_string(&args.migration_definition)?)?;

    let epoch_start = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)?
        .as_secs();
    std::fs::copy(
        &args.migration_definition,
        format!("{}.{}.bck", &args.migration_definition, epoch_start),
    )?;

    debug!("targets: {:?}", &migration_input.targets);
    debug!("definition: {:?}", &migration_input);

    info!("Processing {} repos", &migration_input.targets.len());

    create_dir_all(&args.work_directory_root)?;
    let work_directory_root = args.work_directory_root;

    let github_client = GithubApiClient::new(&args.github_token)?;
    let result_map = Arc::new(Mutex::new(BTreeMap::default()));

    let mut tasks = Vec::new();
    for (pretty_name, target) in &migration_input.targets {
        tasks.push((
            result_map.clone(),
            prepare_migration(
                &migration_input.definition,
                &github_client,
                &args.dry_run_opts,
                &work_directory_root,
                pretty_name,
                target,
            )
            .await?,
        ));
    }

    stream::iter(tasks)
        .for_each_concurrent(3, |(result_map, task)| async move {
            let migration_status = task.run().await;
            let mut result_map = result_map.lock().unwrap();
            result_map.insert(task.pretty_name, migration_status);
        })
        .await;

    let mut error_log = Vec::default();
    let result_map = result_map.lock().unwrap();
    for (pretty_name, status) in result_map.iter() {
        let status = &status;

        match status {
            MigrationStatus::PullRequest(result) => match &result.result {
                Err(e) => {
                    warn!("{}: Unable to run migration because of {}", pretty_name, e);
                    error_log.push(format!(
                        "{}: Unable to run migration because of {}",
                        pretty_name, e
                    ));
                }
                Ok(pr) => {
                    migration_input
                        .targets
                        .get_mut(pretty_name)
                        .unwrap()
                        .pull_request = Some(pr.clone());
                }
            },
            MigrationStatus::EmptyResponse(result) => match &result.result {
                Err(e) => {
                    warn!("{}: Unable to run migration because: {}", pretty_name, e);
                    error_log.push(format!(
                        "{}: Unable to run migration because: {}",
                        pretty_name, e
                    ));
                }
                Ok(_) => {
                    info!(
                        "{}: Exited successfully with step `{}`",
                        pretty_name, result.name
                    );
                }
            },
        }
    }

    let updated_migration_input = &toml::to_string_pretty(&migration_input)?;
    let mut results = File::create(args.migration_definition)?;
    results.write_all(updated_migration_input.as_bytes())?;

    if !error_log.is_empty() {
        let mut error_results = File::create("migration.errors.txt")?;
        error_results.write_all(error_log.join("\n").as_bytes())?;
        error!("Created migration.errors.txt with the summary of errors");
    }

    Ok(())
}

#[allow(clippy::needless_lifetimes)]
async fn prepare_migration<'a>(
    definition: &MigrationDefinition,
    github_client: &'a GithubApiClient,
    dry_run_opts: &DryRunOpts,
    work_directory_root: &str,
    pretty_name: &str,
    target: &TargetDescription,
) -> anyhow::Result<MigrationTask<'a>> {
    debug!("Processing {:?}", &pretty_name);
    let work_dir = PathBuf::from(&work_directory_root);

    let env = match &target.env {
        Some(value) => value.clone(),
        None => std::collections::BTreeMap::default(),
    };

    let exec_options = ExecutionOptions {
        skip_pull_request: dry_run_opts.skip_pull_request,
        skip_push: dry_run_opts.skip_push,
        dry_run: dry_run_opts.dry_run,
        work_dir,
        env,
        github_client,
    };

    let github_repo = match clu::github::extract_github_info(&target.repo) {
        Ok(repo) => repo,
        Err(e) => anyhow::bail!(clu::migration::MigrationError::InvalidGitRepo { source: e }),
    };

    Ok(MigrationTask::new(
        pretty_name,
        github_repo,
        definition.clone(),
        exec_options,
        target.pull_request.clone(),
        target.skip,
    ))
}

fn configure_logging(logging_opts: &LoggingOpts) {
    use tracing_subscriber::FmtSubscriber;

    // a builder for `FmtSubscriber`.
    let subscriber = FmtSubscriber::builder()
        // all spans/events with a level higher than TRACE (e.g, debug, info, warn, etc.)
        // will be written to stdout.
        .with_max_level(logging_opts.to_level())
        .with_target(false)
        .pretty()
        // completes the builder.
        .finish();

    tracing::subscriber::set_global_default(subscriber).expect("setting default subscriber failed");
}
