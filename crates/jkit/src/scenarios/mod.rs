pub mod gap;
pub mod prereqs;

use anyhow::Result;
use clap::Subcommand;
use std::path::PathBuf;

#[derive(Subcommand, Debug)]
pub enum ScenariosCmd {
    /// Detect SB version, install test deps via jkit pom, resolve runtime, ensure compose template.
    Prereqs {
        #[arg(long)]
        apply: bool,
        #[arg(long, default_value = "pom.xml")]
        pom: PathBuf,
    },
    /// List scenarios in test-scenarios.yaml that lack a matching JUnit test method.
    Gap {
        /// Single domain mode (resolves to docs/domains/<domain>/)
        domain: Option<String>,
        /// Run directory; switches gap to multi-domain aggregation.
        #[arg(long)]
        run: Option<PathBuf>,
        /// Root for JUnit test-file search.
        #[arg(long, default_value = "src/test/java/")]
        test_root: PathBuf,
        #[arg(long, default_value = "pom.xml")]
        pom: PathBuf,
    },
}

pub fn run(cmd: ScenariosCmd) -> Result<()> {
    match cmd {
        ScenariosCmd::Prereqs { apply, pom } => prereqs::run(apply, &pom),
        ScenariosCmd::Gap {
            domain,
            run,
            test_root,
            pom,
        } => gap::run(domain.as_deref(), run.as_deref(), &test_root, &pom),
    }
}
