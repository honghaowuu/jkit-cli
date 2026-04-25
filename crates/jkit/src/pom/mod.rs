pub mod add_dep;
pub mod prereqs;
pub mod xml;

use anyhow::Result;
use clap::{Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Subcommand, Debug)]
pub enum PomCmd {
    /// Install a static profile of pom fragments (deps or plugins).
    Prereqs {
        /// One of: testcontainers, compose, jacoco, quality, smart-doc
        #[arg(long)]
        profile: ProfileArg,
        /// Without it: report state, mutate nothing. With it: install missing fragments.
        #[arg(long)]
        apply: bool,
        /// Path to the Maven project file (default: pom.xml in cwd).
        #[arg(long, default_value = "pom.xml")]
        pom: PathBuf,
    },
    /// Add a single dependency to <dependencies>.
    AddDep {
        #[arg(long)]
        group_id: String,
        #[arg(long)]
        artifact_id: String,
        #[arg(long)]
        version: String,
        #[arg(long)]
        scope: Option<String>,
        #[arg(long)]
        apply: bool,
        #[arg(long, default_value = "pom.xml")]
        pom: PathBuf,
    },
}

#[derive(Copy, Clone, Debug, ValueEnum)]
pub enum ProfileArg {
    Testcontainers,
    Compose,
    Jacoco,
    Quality,
    #[value(name = "smart-doc")]
    SmartDoc,
}

impl ProfileArg {
    pub fn name(self) -> &'static str {
        match self {
            ProfileArg::Testcontainers => "testcontainers",
            ProfileArg::Compose => "compose",
            ProfileArg::Jacoco => "jacoco",
            ProfileArg::Quality => "quality",
            ProfileArg::SmartDoc => "smart-doc",
        }
    }
}

pub fn run(cmd: PomCmd) -> Result<()> {
    match cmd {
        PomCmd::Prereqs { profile, apply, pom } => {
            prereqs::run(profile, apply, &pom)
        }
        PomCmd::AddDep {
            group_id,
            artifact_id,
            version,
            scope,
            apply,
            pom,
        } => add_dep::run(&group_id, &artifact_id, &version, scope.as_deref(), apply, &pom),
    }
}
