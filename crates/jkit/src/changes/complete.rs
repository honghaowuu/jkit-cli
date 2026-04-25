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
    git_amended: bool,
}

pub fn run(run: &Path, no_amend: bool) -> Result<()> {
    let cwd = std::env::current_dir().context("reading cwd")?;
    let report = complete(&cwd, run, no_amend)?;
    print_json(&report)
}

pub fn complete(cwd: &Path, run_arg: &Path, no_amend: bool) -> Result<CompleteReport> {
    let run_dir = if run_arg.is_absolute() {
        run_arg.to_path_buf()
    } else {
        cwd.join(run_arg)
    };
    if !run_dir.is_dir() {
        anyhow::bail!("run dir not found: {}", run_dir.display());
    }
    let change_files_path = run_dir.join(".change-files");
    if !change_files_path.is_file() {
        anyhow::bail!(".change-files missing in {}", run_dir.display());
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
    let done_root = cwd.join(".jkit").join("done");
    fs::create_dir_all(&done_root)
        .with_context(|| format!("creating {}", done_root.display()))?;
    let run_name = run_dir
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("run dir has no basename"))?;
    let archived = done_root.join(run_name);
    if archived.exists() {
        anyhow::bail!(
            "archive collision at {} — run already completed?",
            archived.display()
        );
    }
    fs::rename(&run_dir, &archived).with_context(|| {
        format!("archiving {} -> {}", run_dir.display(), archived.display())
    })?;

    let git_amended = if no_amend {
        false
    } else {
        let docs_changes = cwd.join("docs/changes/");
        let jkit_done = cwd.join(".jkit/done/");
        git_add(&docs_changes);
        git_add(&jkit_done);
        git_amend()
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
        git_amended,
    })
}

fn git_add(path: &Path) -> bool {
    let out = Command::new("git").arg("add").arg(path).output();
    matches!(out, Ok(o) if o.status.success())
}

fn git_amend() -> bool {
    let out = Command::new("git")
        .args(["commit", "--amend", "--no-edit"])
        .output();
    match out {
        Ok(o) if o.status.success() => true,
        Ok(o) => {
            eprintln!(
                "warning: git commit --amend failed: {}",
                String::from_utf8_lossy(&o.stderr)
            );
            false
        }
        Err(e) => {
            eprintln!("warning: git commit --amend: {}", e);
            false
        }
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
        let report = complete(tmp.path(), &run, true).unwrap();
        assert_eq!(report.moved, vec!["a.md", "b.md"]);
        assert!(report.already_done.is_empty());
        assert!(report.missing.is_empty());
        assert!(!report.git_amended);
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
        let report = complete(tmp.path(), &run, true).unwrap();
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
        let report = complete(tmp.path(), &run, true).unwrap();
        assert_eq!(report.missing, vec!["ghost.md"]);
    }

    #[test]
    fn refuses_when_archive_collides() {
        let tmp = tempdir().unwrap();
        let run = setup(tmp.path(), &["a.md"]);
        fs::create_dir_all(tmp.path().join(".jkit/done/2026-04-25-feat")).unwrap();
        let err = complete(tmp.path(), &run, true).unwrap_err();
        assert!(err.to_string().contains("archive collision"));
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
        let err = complete(tmp.path(), &run, true).unwrap_err();
        assert!(err.to_string().contains(".change-files missing"));
    }
}
