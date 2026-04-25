pub mod config;

use anyhow::Result;
use clap::Subcommand;

#[derive(Subcommand, Debug)]
pub enum StandardsCmd {
    /// Print the absolute paths of standards files that apply to this project.
    List {
        /// Show gate decisions next to each file.
        #[arg(long)]
        explain: bool,
    },
    /// Create docs/project-info.yaml from the shipped template if it does not exist.
    Init {
        /// Overwrite docs/project-info.yaml if it already exists.
        #[arg(long)]
        force: bool,
    },
}

pub fn run(cmd: StandardsCmd) -> Result<()> {
    match cmd {
        StandardsCmd::List { explain: _ } => {
            anyhow::bail!("not implemented yet (Task 4 / 5)")
        }
        StandardsCmd::Init { force: _ } => {
            anyhow::bail!("not implemented yet (Task 6)")
        }
    }
}
