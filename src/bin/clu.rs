use clap::{ArgGroup, Clap};
use std::fs::{create_dir_all, read_to_string, remove_dir_all, File};
use std::io::Write;
use std::path::PathBuf;

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
    Init,
    RunMigration(RunMigrationArgs),
    CheckStatus(CheckStatusArgs)
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
    /// A JSON file that contains a list of repo's to nice names.
    #[clap(long)]
    pub targets: String,

    /// A TOML file that contains the steps required to do a migration.
    #[clap(long)]
    pub definition: String,

    /// When a repo is migrated, it will be written into this file so other commands can use them.
    #[clap(long)]
    pub results: String,

    /// Folder where the work will take place
    #[clap(long)]
    pub work_directory_root: String,

    /// Token to be used when talking to GitHub
    #[clap(long, env = "GITHUB_TOKEN")]
    pub github_token: String,
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

    return match opt.sub_command {
        SubCommand::Init => run_init().await,
        SubCommand::RunMigration(args) => run_migration(args).await,
        SubCommand::CheckStatus(args) => check_status(args).await,
    };
}

async fn check_status(args: CheckStatusArgs) -> AnyResult<()> {
    use clu::github::PullStatus;

    let mut checks_failed: Vec<String> = Vec::new();
    let mut not_approved: Vec<String> = Vec::new();
    let mut mergeable: Vec<String> = Vec::new();
    let mut merged: Vec<String> = Vec::new();

    let results: CreatedPullRequests = toml::from_str(&read_to_string(args.results)?)?;
    for pull in results.pulls {
        let status = clu::github::fetch_pull_status(&args.github_token, &pull).await?;

        match status {
            PullStatus::ChecksFailed => checks_failed.push(pull.to_url()),
            PullStatus::NeedsApproval => not_approved.push(pull.to_url()),
            PullStatus::Mergeable=> mergeable.push(pull.to_url()),
            PullStatus::Merged=> mergeable.push(pull.to_url()),
        }
    }

    checks_failed.sort();
    not_approved.sort();
    mergeable.sort();
    merged.sort();

    println!("# Migration Results
## Checks Failed
{}

## Not Approved
{}

## Mergeable
{}

## Merged
{}", checks_failed.join("\n"), not_approved.join("\n"), mergeable.join("\n"), merged.join("\n"));

    Ok(())
}

async fn run_init() -> AnyResult<()> {
    let target_config = MigrationTargetsConfig::new(vec![MigrationTarget::new(
        "dummy-repo",
        "git@github.com:ethankhall/dummy-repo.git",
    )]);
    let definition = MigrationDefinition {
        checkout: RepoCheckout { 
            branch_name: "ethankhall/foo-example".to_owned(),
            pre_flight: "/usr/bin/true".to_owned(),
        },
        pr: PrCreationDetails {
            title: "Example Title".to_owned(),
            description: "/usr/bin/env cat --help".to_owned(),
        },
        steps: vec![MigrationStep {
            name: "Example".to_owned(),
            migration_script: "examples/example-migration.sh".to_owned(),
        }]
    };

    let target_config = serde_json::to_string_pretty(&target_config)?;

    let mut f = File::create("migration-targets.json")?;
    f.write_all(target_config.as_bytes())
        .expect("Unable to write data");

    let definition = toml::to_string_pretty(&definition)?;

    let mut f = File::create("migration-definition.toml")?;
    f.write_all(definition.as_bytes())
        .expect("Unable to write data");

    Ok(())
}

pub async fn run_migration(args: RunMigrationArgs) -> AnyResult<()> {
    let targets: MigrationTargetsConfig = serde_json::from_str(&read_to_string(&args.targets)?)?;
    let definition: MigrationDefinition = toml::from_str(&read_to_string(&args.definition)?)?;

    debug!("targets: {:?}", &targets);
    debug!("definition: {:?}", &definition);

    info!("Processing {} repos", &targets.targets.len());

    create_dir_all(&args.work_directory_root)?;
    let work_directory_root = args.work_directory_root;
    let mut created_pull_requests = Vec::new();

    for target in targets.targets {
        debug!("Processing {:?}", target);
        let target_dir = PathBuf::from(&work_directory_root).join(&target.pretty_name);
        if target_dir.exists() {
            remove_dir_all(&target_dir)?;
        }
        create_dir_all(&target_dir)?;

        let input = MigrationInput {
            target: target.clone(),
            definition: definition.clone(),
            work_dir: target_dir.clone(),
            github_token: args.github_token.clone(),
        };

        match clu::migration::run_migration(input).await {
            Ok(pull) => created_pull_requests.push(pull),
            Err(e) => warn!("There was a problem migration {}. Err: {:?}", target.pretty_name, e)
        };
    }

    let created_prs = toml::to_string_pretty(&CreatedPullRequests { pulls: created_pull_requests})?;
    let mut results = File::create(args.results)?;
    results.write_all(created_prs.as_bytes())?;

    Ok(())
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
