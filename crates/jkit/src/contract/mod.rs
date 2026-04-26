pub mod service_meta;
pub mod stage;
pub mod stage_status;

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
    /// Read-only: report whether `.jkit/contract-stage/<service>/` has been
    /// hand-edited since the last successful `stage`. Compares current
    /// SHA256s of every file in `.manifest.json` to the recorded hashes.
    /// Used by the publish-contract skill to escalate the overwrite gate
    /// when a re-stage would clobber local edits.
    StageStatus {
        #[arg(long)]
        service: String,
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
        ContractCmd::StageStatus { service } => stage_status::run(&service),
    }
}
