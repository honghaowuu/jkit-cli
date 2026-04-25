pub mod diff;
pub mod place;

use anyhow::Result;
use clap::Subcommand;
use std::path::PathBuf;

#[derive(Subcommand, Debug)]
pub enum MigrationCmd {
    /// Compute the schema delta and surface warnings.
    Diff {
        #[arg(long)]
        run: PathBuf,
        #[arg(long)]
        no_db: bool,
        #[arg(long, default_value = "pom.xml")]
        pom: PathBuf,
        #[arg(long)]
        target: Option<PathBuf>,
    },
    /// Move an approved SQL file into the Flyway directory with a freshly-computed NNN.
    Place {
        #[arg(long)]
        run: PathBuf,
        #[arg(long)]
        feature: String,
        #[arg(long, default_value = "src/main/resources/db/migration/")]
        flyway_dir: PathBuf,
    },
}

pub fn run(cmd: MigrationCmd) -> Result<()> {
    match cmd {
        MigrationCmd::Diff {
            run,
            no_db,
            pom,
            target,
        } => diff::run(&run, no_db, &pom, target.as_deref()),
        MigrationCmd::Place {
            run,
            feature,
            flyway_dir,
        } => place::run(&run, &feature, &flyway_dir),
    }
}
