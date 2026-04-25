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
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.cmd {
        Top::Pom { cmd } => jkit::pom::run(cmd),
        Top::Coverage(args) => jkit::coverage::run(args),
        Top::Scenarios { cmd } => jkit::scenarios::run(cmd),
        Top::Migration { cmd } => jkit::migration::run(cmd),
        Top::Contract { cmd } => jkit::contract::run(cmd),
        Top::Changes { cmd } => jkit::changes::run(cmd),
        Top::Standards { cmd } => jkit::standards::run(cmd),
    };
    match result {
        Ok(()) => ExitCode::from(0),
        Err(e) => {
            eprintln!("error: {}", e);
            for cause in e.chain().skip(1) {
                eprintln!("  caused by: {}", cause);
            }
            ExitCode::from(1)
        }
    }
}
