pub mod service_meta;
pub mod stage;

use anyhow::Result;
use clap::Subcommand;
use std::path::PathBuf;

#[derive(Subcommand, Debug)]
pub enum ContractCmd {
    /// Read-only: returns the metadata the contract skill needs.
    ServiceMeta {
        #[arg(long, default_value = "pom.xml")]
        pom: PathBuf,
        #[arg(long, default_value = "src/main/java/")]
        src: PathBuf,
    },
    /// Generate the contract bundle in `.jkit/contract-stage/<service-name>/`.
    Stage {
        #[arg(long)]
        service: String,
        #[arg(long)]
        interview: PathBuf,
        /// Comma-separated list of confirmed domain slugs.
        #[arg(long)]
        domains: String,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        force: bool,
        #[arg(long, default_value = "pom.xml")]
        pom: PathBuf,
        #[arg(long, default_value = "src/main/java/")]
        src: PathBuf,
    },
}

pub fn run(cmd: ContractCmd) -> Result<()> {
    match cmd {
        ContractCmd::ServiceMeta { pom, src } => service_meta::run(&pom, &src),
        ContractCmd::Stage {
            service,
            interview,
            domains,
            dry_run,
            force,
            pom,
            src,
        } => {
            let domains: Vec<String> = domains
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            stage::run(&service, &interview, &domains, dry_run, force, &pom, &src)
        }
    }
}
