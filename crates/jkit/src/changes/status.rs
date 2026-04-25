use anyhow::{Context, Result};
use serde::Serialize;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::util::print_json;

#[derive(Serialize, Debug)]
pub struct Status {
    pending_files: Vec<String>,
    existing_run: Option<ExistingRun>,
    recommendation: &'static str,
}

#[derive(Serialize, Debug)]
pub struct ExistingRun {
    path: String,
    change_files: Vec<String>,
    new_pending_since_run: Vec<String>,
    has_change_summary: bool,
    has_plan: bool,
}

const REC_NO_PENDING: &str = "no_pending";
const REC_RESUME: &str = "resume";
const REC_START_NEW: &str = "start_new";

pub fn run() -> Result<()> {
    let cwd = std::env::current_dir().context("reading cwd")?;
    let status = compute(&cwd)?;
    print_json(&status)
}

pub fn compute(cwd: &Path) -> Result<Status> {
    let pending_files = list_pending(cwd)?;
    let existing_run = latest_run(cwd, &pending_files)?;

    let recommendation = if pending_files.is_empty() {
        REC_NO_PENDING
    } else if existing_run.is_some() {
        REC_RESUME
    } else {
        REC_START_NEW
    };

    Ok(Status {
        pending_files,
        existing_run,
        recommendation,
    })
}

fn list_pending(cwd: &Path) -> Result<Vec<String>> {
    let dir = cwd.join("docs/changes/pending");
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut files: Vec<String> = fs::read_dir(&dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .filter_map(|e| {
            e.file_name()
                .to_str()
                .filter(|n| n.ends_with(".md"))
                .map(String::from)
        })
        .collect();
    files.sort();
    Ok(files)
}

fn latest_run(cwd: &Path, pending: &[String]) -> Result<Option<ExistingRun>> {
    let jkit_dir = cwd.join(".jkit");
    if !jkit_dir.is_dir() {
        return Ok(None);
    }
    // Date-prefixed run dirs only.
    let re = regex::Regex::new(r"^\d{4}-\d{2}-\d{2}-.+$").unwrap();
    let mut entries: Vec<PathBuf> = fs::read_dir(&jkit_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter(|e| {
            e.file_name()
                .to_str()
                .map(|s| re.is_match(s))
                .unwrap_or(false)
        })
        .map(|e| e.path())
        .collect();
    entries.sort();
    let Some(latest) = entries.pop() else {
        return Ok(None);
    };
    let change_files_path = latest.join(".change-files");
    if !change_files_path.is_file() {
        return Ok(None);
    }
    let change_files: Vec<String> = fs::read_to_string(&change_files_path)?
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect();
    let change_set: HashSet<&str> = change_files.iter().map(|s| s.as_str()).collect();
    let new_pending_since_run: Vec<String> = pending
        .iter()
        .filter(|f| !change_set.contains(f.as_str()))
        .cloned()
        .collect();
    let has_change_summary = latest.join("change-summary.md").is_file();
    let has_plan = latest.join("plan.md").is_file();
    let path_str = latest
        .strip_prefix(cwd)
        .unwrap_or(&latest)
        .to_string_lossy()
        .into_owned();
    Ok(Some(ExistingRun {
        path: path_str,
        change_files,
        new_pending_since_run,
        has_change_summary,
        has_plan,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn write(p: &Path, contents: &str) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, contents).unwrap();
    }

    #[test]
    fn no_pending_no_jkit() {
        let tmp = tempdir().unwrap();
        let s = compute(tmp.path()).unwrap();
        assert_eq!(s.recommendation, "no_pending");
        assert!(s.pending_files.is_empty());
        assert!(s.existing_run.is_none());
    }

    #[test]
    fn pending_files_only_recommends_start_new() {
        let tmp = tempdir().unwrap();
        write(&tmp.path().join("docs/changes/pending/2026-04-24-foo.md"), "x");
        write(&tmp.path().join("docs/changes/pending/2026-04-25-bar.md"), "y");
        let s = compute(tmp.path()).unwrap();
        assert_eq!(s.recommendation, "start_new");
        assert_eq!(s.pending_files, vec!["2026-04-24-foo.md", "2026-04-25-bar.md"]);
        assert!(s.existing_run.is_none());
    }

    #[test]
    fn run_dir_with_change_files_recommends_resume() {
        let tmp = tempdir().unwrap();
        write(&tmp.path().join("docs/changes/pending/2026-04-24-foo.md"), "x");
        write(
            &tmp.path().join(".jkit/2026-04-24-billing/.change-files"),
            "2026-04-24-foo.md\n",
        );
        let s = compute(tmp.path()).unwrap();
        assert_eq!(s.recommendation, "resume");
        let run = s.existing_run.unwrap();
        assert!(run.path.ends_with("2026-04-24-billing"));
        assert_eq!(run.change_files, vec!["2026-04-24-foo.md"]);
        assert!(run.new_pending_since_run.is_empty());
        assert!(!run.has_change_summary);
        assert!(!run.has_plan);
    }

    #[test]
    fn new_pending_since_run_is_set_difference() {
        let tmp = tempdir().unwrap();
        write(&tmp.path().join("docs/changes/pending/old.md"), "x");
        write(&tmp.path().join("docs/changes/pending/fresh.md"), "y");
        write(
            &tmp.path().join(".jkit/2026-04-24-r/.change-files"),
            "old.md\n",
        );
        let s = compute(tmp.path()).unwrap();
        let run = s.existing_run.unwrap();
        assert_eq!(run.new_pending_since_run, vec!["fresh.md"]);
    }

    #[test]
    fn picks_lexicographically_latest_run_dir() {
        let tmp = tempdir().unwrap();
        write(&tmp.path().join("docs/changes/pending/foo.md"), "x");
        write(
            &tmp.path().join(".jkit/2026-04-23-old/.change-files"),
            "foo.md\n",
        );
        write(
            &tmp.path().join(".jkit/2026-04-25-new/.change-files"),
            "foo.md\n",
        );
        let s = compute(tmp.path()).unwrap();
        assert!(s.existing_run.unwrap().path.ends_with("2026-04-25-new"));
    }

    #[test]
    fn ignores_non_date_prefixed_dirs() {
        let tmp = tempdir().unwrap();
        write(&tmp.path().join("docs/changes/pending/foo.md"), "x");
        write(&tmp.path().join(".jkit/scratch/.change-files"), "foo.md\n");
        let s = compute(tmp.path()).unwrap();
        assert!(s.existing_run.is_none());
        assert_eq!(s.recommendation, "start_new");
    }

    #[test]
    fn run_with_summary_and_plan_files_reports_flags() {
        let tmp = tempdir().unwrap();
        write(&tmp.path().join("docs/changes/pending/foo.md"), "x");
        write(
            &tmp.path().join(".jkit/2026-04-24-r/.change-files"),
            "foo.md\n",
        );
        write(&tmp.path().join(".jkit/2026-04-24-r/change-summary.md"), "");
        write(&tmp.path().join(".jkit/2026-04-24-r/plan.md"), "");
        let s = compute(tmp.path()).unwrap();
        let run = s.existing_run.unwrap();
        assert!(run.has_change_summary);
        assert!(run.has_plan);
    }

    #[test]
    fn no_pending_with_existing_run_still_returns_no_pending() {
        let tmp = tempdir().unwrap();
        write(
            &tmp.path().join(".jkit/2026-04-24-r/.change-files"),
            "foo.md\n",
        );
        let s = compute(tmp.path()).unwrap();
        assert_eq!(s.recommendation, "no_pending");
    }
}
