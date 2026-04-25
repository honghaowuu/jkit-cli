pub mod config;
pub mod gates;
pub mod init;
pub mod list;

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
        StandardsCmd::List { explain } => list::run(explain),
        StandardsCmd::Init { force } => init::run(force),
    }
}
