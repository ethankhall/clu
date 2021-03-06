use clap::{ArgGroup, Clap};
use std::fs::{create_dir_all, read_to_string, remove_dir_all, File};
use std::io::Write;
use std::path::PathBuf;
use futures::stream::{self, StreamExt};

use anyhow::Result as AnyResult;
use tracing::{debug, info, warn};

use clu::models::*;

#[derive(Clap, Debug)]
#[clap(author, about, version)]
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
    /// Run a migration, and write the results back to the file
    RunMigration(RunMigrationArgs),
    /// Check the status of a migration
    CheckStatus(CheckStatusArgs),
}

#[derive(Clap, Debug)]
pub struct CheckStatusArgs {
    /// When a repo is migrated, it will be written into this file so other commands can use them.
    #[clap(long)]
    pub results: String,

    /// Token to be used when talking to GitHub
    #[clap(long, env = "GITHUB_TOKEN")]
    pub github_token: String,
}

#[derive(Clap, Debug)]
pub struct RunMigrationArgs {
    /// A TOML file that defines the input needed to run a migration. This file will be updated
    /// with the results of the run.
    #[clap(long)]
    pub migration_defintion: String,

    /// This file is the updated contents of the migration_defintion with
    /// results.
    #[clap(long)]
    pub results_file: String,

    /// Folder where the work will take place
    #[clap(long = "work-directory")]
    pub work_directory_root: String,

    /// Token to be used when talking to GitHub
    #[clap(long, env = "GITHUB_TOKEN")]
    pub github_token: String,

    /// When set, the PR will not be created
    #[clap(long)]
    pub skip_pull_request: bool,
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
    }
}

async fn check_status(args: CheckStatusArgs) -> AnyResult<()> {
    use clu::github::PullStatus;

    let mut checks_failed: Vec<String> = Vec::new();
    let mut not_approved: Vec<String> = Vec::new();
    let mut mergeable: Vec<String> = Vec::new();
    let mut merged: Vec<String> = Vec::new();

    let results: MigrationInput = toml::from_str(&read_to_string(args.results)?)?;
    for (_name, target) in results.targets {
        let pull = match target.migration_status {
            Some(MigrationStatus::PullRequestCreated(pull)) => pull,
            _ => continue
        };

        let status = clu::github::fetch_pull_status(&args.github_token, &pull).await?;

        match status {
            PullStatus::ChecksFailed => checks_failed.push(format!("- {}", pull.to_url())),
            PullStatus::NeedsApproval => not_approved.push(format!("- {}", pull.to_url())),
            PullStatus::Mergeable => mergeable.push(format!("- {}", pull.to_url())),
            PullStatus::Merged => mergeable.push(format!("- {}", pull.to_url())),
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
        steps: vec![MigrationStep {
            name: "Example".to_owned(),
            migration_script: "examples/example-migration.sh".to_owned(),
        }],
    };

    let migration_input = MigrationInput {
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
    use std::sync::{Arc, Mutex};
    use std::collections::BTreeMap;

    let mut migration_input: MigrationInput =
        toml::from_str(&read_to_string(&args.migration_defintion)?)?;

    debug!("targets: {:?}", &migration_input.targets);
    debug!("definition: {:?}", &migration_input);

    info!("Processing {} repos", &migration_input.targets.len());

    create_dir_all(&args.work_directory_root)?;
    let work_directory_root = args.work_directory_root;

    let result_map = Arc::new(Mutex::new(BTreeMap::default()));

    let mut tasks = Vec::new();
    for (pretty_name, target) in &migration_input.targets {   
        tasks.push((result_map.clone(),prepair_migration(&migration_input.definition, &args.github_token, args.skip_pull_request, &work_directory_root, &pretty_name, &target).await?));
    }

    stream::iter(tasks).for_each_concurrent(3, |(result_map, task)| async move {
        let status = match run_single_migration(&task).await {
            Err(e) => MigrationStatus::Other { message: e.to_string() },
            Ok(status) => status
        };
        let mut result_map = result_map.lock().unwrap();
        result_map.insert(task.pretty_name, status);
    }).await;

    let result_map = result_map.lock().unwrap();
    for (pretty_name, status) in result_map.iter() {
        let status = status.clone();
        migration_input.targets.get_mut(pretty_name).unwrap().migration_status = Some(status);
    }

    let updated_migration_input = &toml::to_string_pretty(&migration_input)?;
    let mut results = File::create(args.results_file)?;
    results.write_all(updated_migration_input.as_bytes())?;

    Ok(())
}


async fn prepair_migration(definition: &MigrationDefinition, github_token: &str, skip_pull_request: bool, work_directory_root: &str, pretty_name: &str, target: &TargetDescription) -> anyhow::Result<MigrationTask> {
    debug!("Processing {:?}", &pretty_name);
    let target_dir = PathBuf::from(&work_directory_root).join(&pretty_name);

    let env = match &target.env {
        Some(value) => value.clone(),
        None => std::collections::BTreeMap::default(),
    };

    Ok(MigrationTask {
        pretty_name: pretty_name.to_owned(),
        repo: target.repo.clone(),
        definition: definition.clone(),
        work_dir: target_dir.clone(),
        env,
        github_token: github_token.to_owned(),
        dry_run: skip_pull_request,
    })
}

async fn run_single_migration(input: &MigrationTask) -> anyhow::Result<MigrationStatus> {
    debug!("Processing {:?}", &input.pretty_name);
    if input.work_dir.exists() {
        remove_dir_all(&input.work_dir)?;
    }
    create_dir_all(&input.work_dir)?;

    match clu::migration::run_migration(&input).await {
        Ok(pull) => return Ok(pull.into()),
        Err(e) => {
            warn!(
                "There was a problem migration {}. Err: {:?}",
                &input.pretty_name, e
            );

            return Ok(MigrationStatus::Other {
                message: e.to_string()
            });
        }
    };
}

fn configure_logging(logging_opts: &LoggingOpts) {
    use tracing_subscriber::{fmt::format::FmtSpan, FmtSubscriber};

    // a builder for `FmtSubscriber`.
    let subscriber = FmtSubscriber::builder()
        // all spans/events with a level higher than TRACE (e.g, debug, info, warn, etc.)
        // will be written to stdout.
        .with_max_level(logging_opts.to_level())
        // Record an event when each span closes. This can be used to time our
        // routes' durations!
        .with_span_events(FmtSpan::CLOSE)
        // completes the builder.
        .finish();

    tracing::subscriber::set_global_default(subscriber).expect("setting default subscriber failed");
}
