pub mod report;

use anyhow::Result;
use clap::Subcommand;
use std::path::PathBuf;

#[derive(Subcommand, Debug)]
pub enum ConflictsCmd {
    /// Detect drift between request/response DTOs and JPA entities — type
    /// mismatches and validation gaps. Read-only. Output is consumed by
    /// `/migrate-project` (writes per-domain `conflicts.md`) and may surface
    /// in `/post-edit`'s remediation routing alongside `jkit drift report`.
    Report {
        /// Optional informational tag for the output `domains[].name` field.
        /// Filtering by domain requires a `--domain-map`; without one, all
        /// detected conflicts are reported under this single label.
        domain: Option<String>,
        #[arg(long, default_value = "src/main/java")]
        src: PathBuf,
        #[arg(long, default_value = "pom.xml")]
        pom: PathBuf,
        /// Optional `.jkit/migration-domain-map.json`. When set, conflicts
        /// are grouped per-domain by entity-package proximity to the
        /// controllers listed under each slug.
        #[arg(long)]
        domain_map: Option<PathBuf>,
    },
}

pub fn run(cmd: ConflictsCmd) -> Result<()> {
    match cmd {
        ConflictsCmd::Report {
            domain,
            src,
            pom,
            domain_map,
        } => report::run(domain.as_deref(), &src, &pom, domain_map.as_deref()),
    }
}
