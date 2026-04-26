pub mod report;

use anyhow::Result;
use clap::Subcommand;
use std::path::PathBuf;

#[derive(Subcommand, Debug)]
pub enum DriftCmd {
    /// Report bi-directional drift between controllers and api-spec.yaml.
    /// Code-without-spec entries route to /write-change → /spec-delta;
    /// spec-without-code entries are either unimplemented endpoints or
    /// stale spec — the human decides via the same route.
    Report {
        /// Restrict to a single domain (matches the resolved slug from
        /// `--domain-map` if present, else `derive_domain_slug`). Omit to
        /// report drift across every detected domain.
        domain: Option<String>,
        #[arg(long, default_value = "src/main/java")]
        src: PathBuf,
        #[arg(long, default_value = "pom.xml")]
        pom: PathBuf,
        #[arg(long, default_value = "docs/domains")]
        domains_root: PathBuf,
        /// Optional `.jkit/migration-domain-map.json`. When set, controllers
        /// are bucketed by the resolved slug from the file; otherwise each
        /// controller's `derive_domain_slug` (Controller-class-name based)
        /// is used. Match scaffold-docs with this flag whenever the project
        /// has run `infer-domains` + a domain-map review.
        #[arg(long)]
        domain_map: Option<PathBuf>,
    },
}

pub fn run(cmd: DriftCmd) -> Result<()> {
    match cmd {
        DriftCmd::Report {
            domain,
            src,
            pom,
            domains_root,
            domain_map,
        } => report::run(
            domain.as_deref(),
            &src,
            &pom,
            &domains_root,
            domain_map.as_deref(),
        ),
    }
}
