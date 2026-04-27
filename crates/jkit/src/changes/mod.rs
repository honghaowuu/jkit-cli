pub mod bootstrap;
pub mod complete;
pub mod doctor;
pub mod init;
pub mod status;
pub mod summary;
pub mod validate;

use anyhow::Result;
use clap::Subcommand;
use std::path::PathBuf;

#[derive(Subcommand, Debug)]
pub enum ChangesCmd {
    /// Create `docs/changes/{pending,done}/` (with .gitkeep). Idempotent.
    Bootstrap,
    /// Report pending change files and any in-progress run.
    Status,
    /// Validate change files: non-empty body and frontmatter `domain:` (if set) exists.
    Validate {
        /// Specific files (relative to cwd or absolute). Default: all under docs/changes/pending/.
        #[arg(long, value_delimiter = ',')]
        files: Option<Vec<PathBuf>>,
    },
    /// Create `.jkit/<date>-<feature>/` and write `.change-files`. Idempotent on identical input.
    Init {
        /// Slug for the run dir. Optional when `--files` lists exactly one change file
        /// (slug is then derived from the file's basename, stripping any leading
        /// `YYYY-MM-DD-` and the `.md` suffix). Required for multi-file runs.
        #[arg(long)]
        feature: Option<String>,
        /// Comma-separated change-file basenames to record.
        #[arg(long, value_delimiter = ',', required = true)]
        files: Vec<String>,
        /// Date prefix override (default: today, format YYYY-MM-DD).
        #[arg(long)]
        date: Option<String>,
    },
    /// Emit a `change-summary.md` skeleton into the run dir with deterministic fields filled.
    Summary {
        #[arg(long)]
        run: PathBuf,
        #[arg(long)]
        feature: String,
        /// Total unimplemented scenarios across all affected domains.
        #[arg(long, default_value_t = 0)]
        gap_total: u32,
        /// Number of affected domains with at least one gap.
        #[arg(long, default_value_t = 0)]
        gap_domains: u32,
        #[arg(long)]
        date: Option<String>,
    },
    /// Close a run: move .change-files entries from pending/ to done/, archive the run
    /// dir to .jkit/done/, stage the changes, and amend HEAD.
    Complete {
        #[arg(long)]
        run: PathBuf,
        /// Skip the `git add` + `git commit --amend` step (useful for tests).
        #[arg(long)]
        no_amend: bool,
    },
    /// Read-only diagnostic: detect inconsistencies between .change-files,
    /// docs/changes/{pending,done}/, and active run dirs.
    Doctor,
}

pub fn run(cmd: ChangesCmd) -> Result<()> {
    match cmd {
        ChangesCmd::Bootstrap => bootstrap::run(),
        ChangesCmd::Status => status::run(),
        ChangesCmd::Validate { files } => validate::run(files),
        ChangesCmd::Init {
            feature,
            files,
            date,
        } => init::run(feature.as_deref(), &files, date.as_deref()),
        ChangesCmd::Summary {
            run,
            feature,
            gap_total,
            gap_domains,
            date,
        } => summary::run(summary::SummaryArgs {
            run,
            feature,
            gap_total,
            gap_domains,
            date,
        }),
        ChangesCmd::Complete { run, no_amend } => complete::run(&run, no_amend),
        ChangesCmd::Doctor => doctor::run(),
    }
}
