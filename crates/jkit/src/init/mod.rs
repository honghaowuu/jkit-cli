pub mod scaffold;

use anyhow::{Context, Result};
use clap::Subcommand;
use serde::Serialize;

use crate::standards::init::{self as standards_init, StandardsInitReport};
use crate::util::print_json;

#[derive(Subcommand, Debug)]
pub enum InitCmd {
    /// Copy user-owned scaffold files (.envrc, .env/{local,test}.env,
    /// docker-compose.yml, docs/overview.md) into the project, idempotently.
    /// Also merges canonical entries into .gitignore under a fenced jkit block.
    Scaffold,
}

#[derive(Serialize, Debug)]
struct InitReport {
    steps: Steps,
    next_steps: Vec<String>,
}

#[derive(Serialize, Debug)]
struct Steps {
    standards_init: StandardsInitReport,
    scaffold: scaffold::ScaffoldReport,
}

/// `jkit init` umbrella: runs `standards init` + `scaffold`, merges per-step
/// reports, prints a single JSON document with `next_steps`.
pub fn run_umbrella() -> Result<()> {
    let cwd = std::env::current_dir().context("reading cwd")?;

    let standards_init = standards_init::init(&cwd, false)?;
    let scaffold = scaffold::scaffold(&cwd)?;

    let next_steps = compose_next_steps(&standards_init, &scaffold);

    let report = InitReport {
        steps: Steps {
            standards_init,
            scaffold,
        },
        next_steps,
    };
    print_json(&report)
}

pub fn run(cmd: InitCmd) -> Result<()> {
    match cmd {
        InitCmd::Scaffold => scaffold::run(),
    }
}

fn compose_next_steps(
    standards_init: &StandardsInitReport,
    scaffold: &scaffold::ScaffoldReport,
) -> Vec<String> {
    let mut steps = Vec::new();
    if !standards_init.created.is_empty() {
        steps.push(
            "Review docs/project-info.yaml and adjust enabled flags".to_string(),
        );
    }
    if scaffold.created.iter().any(|p| p == "docs/overview.md") {
        steps.push("Fill TODOs in docs/overview.md".to_string());
    }
    if scaffold
        .created
        .iter()
        .any(|p| p.starts_with(".env/") || p == ".envrc")
    {
        steps.push(
            "Fill commented values in .env/local.env and .env/test.env".to_string(),
        );
    }
    steps.push(
        "Run /start-feature to begin a new feature".to_string(),
    );
    steps
}
