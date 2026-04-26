use clap::{Parser, Subcommand};
use std::process::ExitCode;

#[derive(Parser, Debug)]
#[command(name = "jkit", version, about = "Java-specific developer pipeline CLI")]
struct Cli {
    #[command(subcommand)]
    cmd: Top,
}

#[derive(Subcommand, Debug)]
enum Top {
    Pom {
        #[command(subcommand)]
        cmd: jkit::pom::PomCmd,
    },
    Coverage(jkit::coverage::CoverageArgs),
    Scenarios {
        #[command(subcommand)]
        cmd: jkit::scenarios::ScenariosCmd,
    },
    Migrate {
        #[command(subcommand)]
        cmd: jkit::migrate::MigrateCmd,
    },
    Migration {
        #[command(subcommand)]
        cmd: jkit::migration::MigrationCmd,
    },
    Contract {
        #[command(subcommand)]
        cmd: jkit::contract::ContractCmd,
    },
    Changes {
        #[command(subcommand)]
        cmd: jkit::changes::ChangesCmd,
    },
    Standards {
        #[command(subcommand)]
        cmd: jkit::standards::StandardsCmd,
    },
    /// One-shot project bootstrap: changes bootstrap + standards init + scaffold.
    /// Idempotent — re-running completes only what's missing.
    Init {
        #[command(subcommand)]
        cmd: Option<jkit::init::InitCmd>,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.cmd {
        Top::Pom { cmd } => jkit::pom::run(cmd),
        Top::Coverage(args) => jkit::coverage::run(args),
        Top::Scenarios { cmd } => jkit::scenarios::run(cmd),
        Top::Migrate { cmd } => jkit::migrate::run(cmd),
        Top::Migration { cmd } => jkit::migration::run(cmd),
        Top::Contract { cmd } => jkit::contract::run(cmd),
        Top::Changes { cmd } => jkit::changes::run(cmd),
        Top::Standards { cmd } => jkit::standards::run(cmd),
        Top::Init { cmd } => match cmd {
            Some(c) => jkit::init::run(c),
            None => jkit::init::run_umbrella(),
        },
    };
    match result {
        Ok(()) => ExitCode::from(0),
        Err(e) => jkit::envelope::print_err(&format!("{e:#}"), None),
    }
}
