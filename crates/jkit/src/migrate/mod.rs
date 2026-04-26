pub mod scaffold_docs;

use anyhow::Result;
use clap::Subcommand;
use std::path::PathBuf;

#[derive(Subcommand, Debug)]
pub enum MigrateCmd {
    /// Reverse-engineer skeleton docs/domains/<n>/ files from existing controllers.
    /// Idempotent: writes only files that don't already exist; surfaces
    /// already-present files in the JSON output. Controllers are grouped by the
    /// same `derive_domain_slug` rule the contract pipeline uses, so re-runs
    /// after `/publish-contract` won't fork the slug naming.
    ScaffoldDocs {
        #[arg(long, default_value = "src/main/java")]
        src: PathBuf,
        #[arg(long, default_value = "pom.xml")]
        pom: PathBuf,
        #[arg(long, default_value = "docs/domains")]
        out: PathBuf,
    },
}

pub fn run(cmd: MigrateCmd) -> Result<()> {
    match cmd {
        MigrateCmd::ScaffoldDocs { src, pom, out } => scaffold_docs::run(&src, &pom, &out),
    }
}
