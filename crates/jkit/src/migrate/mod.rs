pub mod infer_domains;
pub mod smartdoc;

use anyhow::Result;
use clap::Subcommand;
use std::path::PathBuf;

#[derive(Subcommand, Debug)]
pub enum MigrateCmd {
    /// Detect controllers and emit candidate `(slug, [controllers])` groupings
    /// for human review. Output is consumed by `/migrate-project` to build
    /// `.jkit/migration-domain-map.json`, the authoritative mapping that drives
    /// every downstream backfill step. Read-only.
    InferDomains {
        #[arg(long, default_value = "src/main/java")]
        src: PathBuf,
        #[arg(long, default_value = "pom.xml")]
        pom: PathBuf,
    },
}

pub fn run(cmd: MigrateCmd) -> Result<()> {
    match cmd {
        MigrateCmd::InferDomains { src, pom } => infer_domains::run(&src, &pom),
    }
}
