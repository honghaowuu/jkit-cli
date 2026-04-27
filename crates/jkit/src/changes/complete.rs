use anyhow::{Context, Result};
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::util::print_json;

#[derive(Serialize, Debug)]
pub struct CompleteReport {
    moved: Vec<String>,
    already_done: Vec<String>,
    missing: Vec<String>,
    archived_run_dir: String,
    /// `true` when this invocation created a `chore(complete): <feature>` commit.
    /// `false` when there was nothing to commit (re-run after a successful run)
    /// or when `--no-commit` was passed.
    git_committed: bool,
    /// Populated when one of the git steps (add or commit) fails. The file
    /// moves and the run-dir archive are independent of git, so a git failure
    /// surfaces as a non-fatal warning rather than failing the whole command.
    /// Multiple step errors are joined with `; `.
    #[serde(skip_serializing_if = "Option::is_none")]
    git_error: Option<String>,
}

pub fn run(run: &Path, no_commit: bool) -> Result<()> {
    let cwd = std::env::current_dir().context("reading cwd")?;
    let report = complete(&cwd, run, no_commit)?;
    print_json(&report)
}

pub fn complete(cwd: &Path, run_arg: &Path, no_commit: bool) -> Result<CompleteReport> {
    let run_dir_input = if run_arg.is_absolute() {
        run_arg.to_path_buf()
    } else {
        cwd.join(run_arg)
    };

    // Resumability: a previous invocation may have already archived the run
    // dir but failed before (or skipped) the git amend. If the source run dir
    // is gone but the archived copy exists, treat the move as already done.
    let done_root = cwd.join(".jkit").join("done");
    let run_name = run_dir_input
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("run dir has no basename"))?
        .to_owned();
    let archived = done_root.join(&run_name);

    let resuming_from_archived = !run_dir_input.exists() && archived.is_dir();
    let active_run_dir = if resuming_from_archived { &archived } else { &run_dir_input };

    if !active_run_dir.is_dir() {
        anyhow::bail!("run dir not found: {}", run_dir_input.display());
    }
    let change_files_path = active_run_dir.join(".change-files");
    if !change_files_path.is_file() {
        anyhow::bail!(".change-files missing in {}", active_run_dir.display());
    }

    let entries: Vec<String> = fs::read_to_string(&change_files_path)
        .with_context(|| format!("reading {}", change_files_path.display()))?
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect();

    let pending_dir = cwd.join("docs/changes/pending");
    let done_dir = cwd.join("docs/changes/done");
    fs::create_dir_all(&done_dir)
        .with_context(|| format!("creating {}", done_dir.display()))?;

    let mut moved = Vec::new();
    let mut already_done = Vec::new();
    let mut missing = Vec::new();

    for fname in &entries {
        let src = pending_dir.join(fname);
        let dst = done_dir.join(fname);
        if src.is_file() {
            fs::rename(&src, &dst)
                .with_context(|| format!("renaming {} -> {}", src.display(), dst.display()))?;
            moved.push(fname.clone());
        } else if dst.is_file() {
            already_done.push(fname.clone());
        } else {
            missing.push(fname.clone());
        }
    }

    // Archive run dir under .jkit/done/.
    fs::create_dir_all(&done_root)
        .with_context(|| format!("creating {}", done_root.display()))?;
    if resuming_from_archived {
        // Already archived by a prior invocation — skip the rename and
        // proceed to the git step below.
    } else if archived.exists() {
        anyhow::bail!(
            "archive collision at {} — both source and archive exist; resolve manually",
            archived.display()
        );
    } else {
        fs::rename(&run_dir_input, &archived).with_context(|| {
            format!("archiving {} -> {}", run_dir_input.display(), archived.display())
        })?;
    }

    let feature = feature_from_run_name(&run_name.to_string_lossy());

    let (git_committed, git_error) = if no_commit {
        (false, None)
    } else {
        let docs_changes = cwd.join("docs/changes/");
        let jkit_done = cwd.join(".jkit/done/");
        let mut errors: Vec<String> = Vec::new();
        if let Err(e) = git_add(&docs_changes) {
            errors.push(format!("git add docs/changes/: {e}"));
        }
        if let Err(e) = git_add(&jkit_done) {
            errors.push(format!("git add .jkit/done/: {e}"));
        }
        // Skip the commit when nothing is staged — happens on a re-run after a
        // prior invocation already committed everything. Treat as success-no-op.
        let committed = match git_has_staged_changes() {
            Ok(false) => false,
            Ok(true) => match git_commit(&feature) {
                Ok(()) => true,
                Err(e) => {
                    errors.push(format!("git commit: {e}"));
                    false
                }
            },
            Err(e) => {
                errors.push(format!("git diff --staged: {e}"));
                false
            }
        };
        (committed, if errors.is_empty() { None } else { Some(errors.join("; ")) })
    };

    Ok(CompleteReport {
        moved,
        already_done,
        missing,
        archived_run_dir: archived
            .strip_prefix(cwd)
            .unwrap_or(&archived)
            .to_string_lossy()
            .into_owned(),
        git_committed,
        git_error,
    })
}

/// Strip a leading `YYYY-MM-DD-` from the run-dir basename to recover the
/// feature slug. If the prefix is missing, return the whole name unchanged.
fn feature_from_run_name(run_name: &str) -> String {
    let bytes = run_name.as_bytes();
    let is_digit = |b: u8| b.is_ascii_digit();
    let dash = |b: u8| b == b'-';
    if bytes.len() > 11
        && is_digit(bytes[0])
        && is_digit(bytes[1])
        && is_digit(bytes[2])
        && is_digit(bytes[3])
        && dash(bytes[4])
        && is_digit(bytes[5])
        && is_digit(bytes[6])
        && dash(bytes[7])
        && is_digit(bytes[8])
        && is_digit(bytes[9])
        && dash(bytes[10])
    {
        run_name[11..].to_string()
    } else {
        run_name.to_string()
    }
}

/// Returns `Ok(())` when `git add <path>` succeeds and `Err(stderr_text)`
/// otherwise. Errors are aggregated into the envelope's `git_error` field
/// rather than dropped to stderr.
fn git_add(path: &Path) -> std::result::Result<(), String> {
    match Command::new("git").arg("add").arg(path).output() {
        Ok(o) if o.status.success() => Ok(()),
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr).trim().to_string();
            let exit = o.status.code().map(|c| c.to_string()).unwrap_or_else(|| "killed".into());
            Err(if stderr.is_empty() {
                format!("git add exited {exit}")
            } else {
                format!("git add exited {exit}: {stderr}")
            })
        }
        Err(e) => Err(format!("git add invocation failed: {e}")),
    }
}

/// Returns `Ok(true)` when there are staged changes ready to commit,
/// `Ok(false)` when staging is empty (no-op commit case), and
/// `Err(stderr_text)` if `git diff` itself failed.
fn git_has_staged_changes() -> std::result::Result<bool, String> {
    match Command::new("git")
        .args(["diff", "--staged", "--quiet"])
        .output()
    {
        // exit 0 → no staged changes; exit 1 → staged changes present
        Ok(o) => match o.status.code() {
            Some(0) => Ok(false),
            Some(1) => Ok(true),
            Some(c) => {
                let stderr = String::from_utf8_lossy(&o.stderr).trim().to_string();
                Err(if stderr.is_empty() {
                    format!("git diff --staged --quiet exited {c}")
                } else {
                    format!("git diff --staged --quiet exited {c}: {stderr}")
                })
            }
            None => Err("git diff --staged --quiet killed by signal".into()),
        },
        Err(e) => Err(format!("git diff invocation failed: {e}")),
    }
}

/// Returns `Ok(())` when `git commit -m "chore(complete): <feature>"` succeeds
/// and `Err(stderr_text)` otherwise.
fn git_commit(feature: &str) -> std::result::Result<(), String> {
    let msg = format!("chore(complete): {feature}");
    match Command::new("git").args(["commit", "-m", &msg]).output() {
        Ok(o) if o.status.success() => Ok(()),
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr).trim().to_string();
            let exit = o.status.code().map(|c| c.to_string()).unwrap_or_else(|| "killed".into());
            Err(if stderr.is_empty() {
                format!("git commit exited {exit}")
            } else {
                format!("git commit exited {exit}: {stderr}")
            })
        }
        Err(e) => Err(format!("git commit invocation failed: {e}")),
    }
}

// Helper for tests: list of paths the binary expects under cwd.
#[allow(dead_code)]
pub(crate) fn fixture_paths(run_dir: &Path) -> (PathBuf, PathBuf) {
    (run_dir.join(".change-files"), run_dir.join("plan.md"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn write(p: &Path, s: &str) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, s).unwrap();
    }

    fn setup(cwd: &Path, change_file_basenames: &[&str]) -> PathBuf {
        let run = cwd.join(".jkit/2026-04-25-feat");
        fs::create_dir_all(&run).unwrap();
        let cf = run.join(".change-files");
        let body = change_file_basenames.join("\n") + "\n";
        fs::write(cf, body).unwrap();
        for n in change_file_basenames {
            write(&cwd.join(format!("docs/changes/pending/{}", n)), "x");
        }
        run
    }

    #[test]
    fn moves_pending_to_done_and_archives_run() {
        let tmp = tempdir().unwrap();
        let run = setup(tmp.path(), &["a.md", "b.md"]);
        let report = complete(tmp.path(), &run, /* no_commit */ true).unwrap();
        assert_eq!(report.moved, vec!["a.md", "b.md"]);
        assert!(report.already_done.is_empty());
        assert!(report.missing.is_empty());
        assert!(!report.git_committed);
        assert!(tmp.path().join("docs/changes/done/a.md").is_file());
        assert!(tmp.path().join("docs/changes/done/b.md").is_file());
        assert!(!run.exists());
        assert!(tmp.path().join(".jkit/done/2026-04-25-feat").is_dir());
        assert!(report.archived_run_dir.ends_with(".jkit/done/2026-04-25-feat"));
    }

    #[test]
    fn idempotent_when_files_already_in_done() {
        let tmp = tempdir().unwrap();
        let run = setup(tmp.path(), &["a.md"]);
        // Simulate: file already moved by an earlier interrupted run.
        fs::create_dir_all(tmp.path().join("docs/changes/done")).unwrap();
        fs::rename(
            tmp.path().join("docs/changes/pending/a.md"),
            tmp.path().join("docs/changes/done/a.md"),
        )
        .unwrap();
        let report = complete(tmp.path(), &run, /* no_commit */ true).unwrap();
        assert!(report.moved.is_empty());
        assert_eq!(report.already_done, vec!["a.md"]);
        assert!(report.missing.is_empty());
    }

    #[test]
    fn reports_missing_files_without_failing() {
        let tmp = tempdir().unwrap();
        let run = setup(tmp.path(), &["ghost.md"]);
        // Remove the pending file before complete runs.
        fs::remove_file(tmp.path().join("docs/changes/pending/ghost.md")).unwrap();
        let report = complete(tmp.path(), &run, /* no_commit */ true).unwrap();
        assert_eq!(report.missing, vec!["ghost.md"]);
    }

    #[test]
    fn refuses_when_archive_collides() {
        let tmp = tempdir().unwrap();
        let run = setup(tmp.path(), &["a.md"]);
        fs::create_dir_all(tmp.path().join(".jkit/done/2026-04-25-feat")).unwrap();
        let err = complete(tmp.path(), &run, /* no_commit */ true).unwrap_err();
        assert!(err.to_string().contains("archive collision"));
    }

    #[test]
    fn resumes_when_only_archive_exists() {
        // Simulate an interrupted prior run: pending file already moved to
        // done/, run dir already archived, but git amend never happened. A
        // re-run should NOT bail; it should pick up the archived run dir,
        // see all files in done already, and report success.
        let tmp = tempdir().unwrap();
        let run = setup(tmp.path(), &["a.md", "b.md"]);
        // Move the pending files to done as if a prior run did the rename.
        fs::create_dir_all(tmp.path().join("docs/changes/done")).unwrap();
        for n in ["a.md", "b.md"] {
            fs::rename(
                tmp.path().join(format!("docs/changes/pending/{n}")),
                tmp.path().join(format!("docs/changes/done/{n}")),
            )
            .unwrap();
        }
        // Move the run dir to .jkit/done/ as if archived.
        fs::create_dir_all(tmp.path().join(".jkit/done")).unwrap();
        fs::rename(&run, tmp.path().join(".jkit/done/2026-04-25-feat")).unwrap();

        let report = complete(tmp.path(), &run, /* no_commit */ true).expect("resume should succeed");
        assert_eq!(report.already_done, vec!["a.md", "b.md"]);
        assert!(report.moved.is_empty());
        assert!(report.archived_run_dir.ends_with(".jkit/done/2026-04-25-feat"));
    }

    #[test]
    fn missing_run_dir_fails() {
        let tmp = tempdir().unwrap();
        let err = complete(tmp.path(), &PathBuf::from(".jkit/nope"), true).unwrap_err();
        assert!(err.to_string().contains("run dir not found"));
    }

    #[test]
    fn missing_change_files_fails() {
        let tmp = tempdir().unwrap();
        let run = tmp.path().join(".jkit/2026-04-25-feat");
        fs::create_dir_all(&run).unwrap();
        let err = complete(tmp.path(), &run, /* no_commit */ true).unwrap_err();
        assert!(err.to_string().contains(".change-files missing"));
    }

    #[test]
    fn feature_slug_strips_date_prefix() {
        assert_eq!(feature_from_run_name("2026-04-25-billing-bulk-invoice"), "billing-bulk-invoice");
        assert_eq!(feature_from_run_name("2026-04-25-x"), "x");
    }

    #[test]
    fn feature_slug_passthrough_when_no_date_prefix() {
        assert_eq!(feature_from_run_name("custom-name"), "custom-name");
        assert_eq!(feature_from_run_name("short"), "short");
    }
}
