//! `jkit feature` — branch-driven entry routing for `/start-feature`.
//!
//! Two subcommands:
//!   - `start` — detect git branch state + run domain doctor (Step 1 of
//!     `/start-feature`).
//!   - `init`  — derive a slug from the title, validate `--domains` against
//!     `docs/domains.yaml`, create `.jkit/<date>-<slug>/`, and rename the
//!     placeholder `feature/<placeholder-…>` branch to `feature/<slug>`.
//!
//! Both commands return small JSON envelopes the skill consumes directly.

pub mod init;
pub mod start;

use anyhow::Result;
use clap::Subcommand;

#[derive(Subcommand, Debug)]
pub enum FeatureCmd {
    /// Detect branch state + run domain doctor. Routes `/start-feature` Step 1.
    Start,
    /// Create the run dir + rename the placeholder branch + validate frontmatter.
    Init {
        /// Short imperative title; the slug is derived from it (kebab-case,
        /// lowercase, alphanumeric + hyphens).
        #[arg(long)]
        title: String,
        /// Comma-separated list of domain slugs (must exist in docs/domains.yaml).
        #[arg(long, value_delimiter = ',', required = true)]
        domains: Vec<String>,
    },
}

pub fn run(cmd: FeatureCmd) -> Result<()> {
    match cmd {
        FeatureCmd::Start => start::run(),
        FeatureCmd::Init { title, domains } => init::run(&title, &domains),
    }
}
