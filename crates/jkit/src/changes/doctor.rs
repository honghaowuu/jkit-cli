use anyhow::{Context, Result};
use serde::Serialize;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::util::print_json;

#[derive(Serialize, Debug)]
pub struct DoctorReport {
    ok: bool,
    findings: Vec<Finding>,
}

#[derive(Serialize, Debug)]
pub struct Finding {
    code: &'static str,
    severity: &'static str, // "issue" | "warning"
    message: String,
    remediation: String,
}

pub fn run() -> Result<()> {
    let cwd = std::env::current_dir().context("reading cwd")?;
    let report = diagnose(&cwd)?;
    print_json(&report)
}

pub fn diagnose(cwd: &Path) -> Result<DoctorReport> {
    let mut findings = Vec::new();

    let pending: HashSet<String> = list_basenames(&cwd.join("docs/changes/pending"))?
        .into_iter()
        .collect();
    let done: HashSet<String> = list_basenames(&cwd.join("docs/changes/done"))?
        .into_iter()
        .collect();
    let active = active_runs(cwd)?;

    // 1. Multiple active runs at top level.
    if active.len() > 1 {
        let names: Vec<String> = active.iter().map(|r| r.rel.clone()).collect();
        findings.push(Finding {
            code: "multiple_active_runs",
            severity: "issue",
            message: format!(
                "Found {} active run dirs at the top level of .jkit/: {}. Only one run should be in flight at a time.",
                names.len(),
                names.join(", ")
            ),
            remediation:
                "Complete or remove all but one (`jkit changes complete --run <dir>` or `rm -rf <dir>`)."
                    .to_string(),
        });
    }

    // 2/3. For each active run: stale (all in done already) or referencing missing files.
    for run in &active {
        if run.change_files.is_empty() {
            findings.push(Finding {
                code: "empty_change_files",
                severity: "issue",
                message: format!(
                    "Run {} has an empty .change-files. The run cannot be resumed or completed.",
                    run.rel
                ),
                remediation: "Recreate it with `jkit changes init --feature <slug> --files <basenames>` or remove the run dir.".to_string(),
            });
            continue;
        }
        let in_done: Vec<&String> = run
            .change_files
            .iter()
            .filter(|f| done.contains(*f))
            .collect();
        if in_done.len() == run.change_files.len() {
            findings.push(Finding {
                code: "stale_active_run",
                severity: "issue",
                message: format!(
                    "Run {} references change files that are all already in docs/changes/done/. The run was likely never completed.",
                    run.rel
                ),
                remediation: format!(
                    "Run `jkit changes complete --run {}` to archive the run dir.",
                    run.rel
                ),
            });
        }
        for f in &run.change_files {
            if !pending.contains(f) && !done.contains(f) {
                findings.push(Finding {
                    code: "missing_change_file",
                    severity: "issue",
                    message: format!(
                        "Run {} references {} but it's neither in docs/changes/pending/ nor docs/changes/done/.",
                        run.rel, f
                    ),
                    remediation:
                        "Restore the file from git history, or edit .change-files to drop the entry."
                            .to_string(),
                });
            }
        }
    }

    // 4. Orphan pending: in pending/ but not in any active run's .change-files,
    //    AND there is at least one active run (otherwise these are awaiting a future
    //    spec-delta cycle, which is normal).
    if !active.is_empty() {
        let referenced: HashSet<&str> = active
            .iter()
            .flat_map(|r| r.change_files.iter().map(|s| s.as_str()))
            .collect();
        let mut pending_sorted: Vec<&String> = pending.iter().collect();
        pending_sorted.sort();
        for f in pending_sorted {
            if !referenced.contains(f.as_str()) {
                findings.push(Finding {
                    code: "orphan_pending",
                    severity: "warning",
                    message: format!(
                        "docs/changes/pending/{} is not referenced by any active run's .change-files. It will be picked up by the next /spec-delta cycle if no active run is resumed.",
                        f
                    ),
                    remediation: "Informational — no action required.".to_string(),
                });
            }
        }
    }

    // 5. Direct edits to docs/domains/ outside an active spec-delta run.
    //    Best-effort via `git status --porcelain` — silently skip if git is unavailable.
    if active.is_empty() {
        if let Some(modified) = git_modified_under(cwd, "docs/domains") {
            if !modified.is_empty() {
                findings.push(Finding {
                    code: "direct_domain_edit",
                    severity: "warning",
                    message: format!(
                        "docs/domains/ has {} uncommitted file(s) and no active spec-delta run is in flight. Direct edits to formal docs should typically go through /spec-delta.",
                        modified.len()
                    ),
                    remediation:
                        "Commit them as out-of-band work, or revert and write a change file in docs/changes/pending/."
                            .to_string(),
                });
            }
        }
    }

    let ok = !findings.iter().any(|f| f.severity == "issue");
    Ok(DoctorReport { ok, findings })
}

/// Run `git status --porcelain -- <subpath>` and return modified paths.
/// Returns None if git is missing or the command fails (not a git repo, etc.).
fn git_modified_under(cwd: &Path, subpath: &str) -> Option<Vec<String>> {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["status", "--porcelain", "--", subpath])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let stdout = String::from_utf8(out.stdout).ok()?;
    let mut paths = Vec::new();
    for line in stdout.lines() {
        // Porcelain format: 2-char status field, space, path. Skip untracked-only rows? Include them.
        if line.len() < 4 {
            continue;
        }
        let path = line[3..].trim().to_string();
        if !path.is_empty() {
            paths.push(path);
        }
    }
    Some(paths)
}

struct ActiveRun {
    rel: String,
    change_files: Vec<String>,
}

fn active_runs(cwd: &Path) -> Result<Vec<ActiveRun>> {
    let jkit = cwd.join(".jkit");
    if !jkit.is_dir() {
        return Ok(Vec::new());
    }
    let re = regex::Regex::new(r"^\d{4}-\d{2}-\d{2}-.+$").unwrap();
    let mut out = Vec::new();
    for entry in fs::read_dir(&jkit)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        if !re.is_match(&name) {
            continue;
        }
        let cf = path.join(".change-files");
        let change_files = if cf.is_file() {
            fs::read_to_string(&cf)?
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(String::from)
                .collect()
        } else {
            Vec::new()
        };
        let rel = path
            .strip_prefix(cwd)
            .unwrap_or(&path)
            .to_string_lossy()
            .into_owned();
        out.push(ActiveRun { rel, change_files });
    }
    out.sort_by(|a, b| a.rel.cmp(&b.rel));
    Ok(out)
}

fn list_basenames(dir: &Path) -> Result<Vec<String>> {
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut out: Vec<String> = fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .filter_map(|e| {
            e.file_name()
                .to_str()
                .filter(|n| n.ends_with(".md"))
                .map(String::from)
        })
        .collect();
    out.sort();
    Ok(out)
}

#[allow(dead_code)]
fn run_dir_path(cwd: &Path, name: &str) -> PathBuf {
    cwd.join(".jkit").join(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn write(p: &Path, s: &str) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, s).unwrap();
    }

    #[test]
    fn clean_project_returns_ok() {
        let tmp = tempdir().unwrap();
        write(&tmp.path().join("docs/changes/pending/foo.md"), "x");
        let report = diagnose(tmp.path()).unwrap();
        assert!(report.ok);
        assert!(report.findings.is_empty());
    }

    #[test]
    fn detects_multiple_active_runs() {
        let tmp = tempdir().unwrap();
        write(&tmp.path().join(".jkit/2026-04-24-a/.change-files"), "a.md\n");
        write(&tmp.path().join(".jkit/2026-04-25-b/.change-files"), "b.md\n");
        write(&tmp.path().join("docs/changes/pending/a.md"), "x");
        write(&tmp.path().join("docs/changes/pending/b.md"), "y");
        let report = diagnose(tmp.path()).unwrap();
        assert!(!report.ok);
        assert!(report
            .findings
            .iter()
            .any(|f| f.code == "multiple_active_runs"));
    }

    #[test]
    fn detects_stale_active_run() {
        let tmp = tempdir().unwrap();
        write(&tmp.path().join(".jkit/2026-04-24-a/.change-files"), "a.md\n");
        write(&tmp.path().join("docs/changes/done/a.md"), "x");
        let report = diagnose(tmp.path()).unwrap();
        assert!(!report.ok);
        let f = report
            .findings
            .iter()
            .find(|f| f.code == "stale_active_run")
            .unwrap();
        assert!(f.remediation.contains("jkit changes complete"));
    }

    #[test]
    fn detects_missing_change_file() {
        let tmp = tempdir().unwrap();
        write(&tmp.path().join(".jkit/2026-04-24-a/.change-files"), "ghost.md\n");
        let report = diagnose(tmp.path()).unwrap();
        assert!(!report.ok);
        assert!(report
            .findings
            .iter()
            .any(|f| f.code == "missing_change_file"));
    }

    #[test]
    fn detects_orphan_pending_only_when_active_run_exists() {
        let tmp = tempdir().unwrap();
        // No active run: pending file is fine (awaiting next spec-delta cycle).
        write(&tmp.path().join("docs/changes/pending/orphan.md"), "x");
        let r1 = diagnose(tmp.path()).unwrap();
        assert!(r1.findings.is_empty());

        // Active run present, doesn't reference orphan.md → warning.
        write(&tmp.path().join(".jkit/2026-04-24-a/.change-files"), "real.md\n");
        write(&tmp.path().join("docs/changes/pending/real.md"), "x");
        let r2 = diagnose(tmp.path()).unwrap();
        assert!(r2.ok); // warning, not issue
        assert!(r2
            .findings
            .iter()
            .any(|f| f.code == "orphan_pending" && f.severity == "warning"));
    }

    #[test]
    fn detects_empty_change_files() {
        let tmp = tempdir().unwrap();
        write(&tmp.path().join(".jkit/2026-04-24-a/.change-files"), "");
        let report = diagnose(tmp.path()).unwrap();
        assert!(!report.ok);
        assert!(report.findings.iter().any(|f| f.code == "empty_change_files"));
    }

    #[test]
    fn ignores_archived_runs_under_done() {
        let tmp = tempdir().unwrap();
        write(
            &tmp.path().join(".jkit/done/2026-04-24-old/.change-files"),
            "old.md\n",
        );
        write(&tmp.path().join("docs/changes/done/old.md"), "x");
        let report = diagnose(tmp.path()).unwrap();
        assert!(report.ok);
        assert!(report.findings.is_empty());
    }

    #[test]
    fn detects_direct_domain_edit_when_no_active_run() {
        // Set up a git repo with a tracked docs/domains/ file modified.
        let tmp = tempdir().unwrap();
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(tmp.path())
            .status()
            .unwrap();
        std::process::Command::new("git")
            .args(["config", "user.email", "t@t"])
            .current_dir(tmp.path())
            .status()
            .unwrap();
        std::process::Command::new("git")
            .args(["config", "user.name", "t"])
            .current_dir(tmp.path())
            .status()
            .unwrap();
        write(&tmp.path().join("docs/domains/billing/api-spec.yaml"), "v: 1\n");
        std::process::Command::new("git")
            .args(["add", "."])
            .current_dir(tmp.path())
            .status()
            .unwrap();
        std::process::Command::new("git")
            .args(["commit", "-q", "-m", "init"])
            .current_dir(tmp.path())
            .status()
            .unwrap();
        // Now modify the file (uncommitted change).
        write(&tmp.path().join("docs/domains/billing/api-spec.yaml"), "v: 2\n");

        let report = diagnose(tmp.path()).unwrap();
        assert!(report.ok); // warning, not issue
        assert!(report
            .findings
            .iter()
            .any(|f| f.code == "direct_domain_edit"));
    }

    #[test]
    fn skips_direct_domain_edit_check_when_active_run_exists() {
        let tmp = tempdir().unwrap();
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(tmp.path())
            .status()
            .unwrap();
        std::process::Command::new("git")
            .args(["config", "user.email", "t@t"])
            .current_dir(tmp.path())
            .status()
            .unwrap();
        std::process::Command::new("git")
            .args(["config", "user.name", "t"])
            .current_dir(tmp.path())
            .status()
            .unwrap();
        write(&tmp.path().join("docs/domains/billing/api-spec.yaml"), "v: 1\n");
        std::process::Command::new("git")
            .args(["add", "."])
            .current_dir(tmp.path())
            .status()
            .unwrap();
        std::process::Command::new("git")
            .args(["commit", "-q", "-m", "init"])
            .current_dir(tmp.path())
            .status()
            .unwrap();
        write(&tmp.path().join("docs/domains/billing/api-spec.yaml"), "v: 2\n");
        // Active run present — direct-edit check should be skipped.
        write(
            &tmp.path().join(".jkit/2026-04-25-feat/.change-files"),
            "x.md\n",
        );
        write(&tmp.path().join("docs/changes/pending/x.md"), "y");

        let report = diagnose(tmp.path()).unwrap();
        assert!(!report
            .findings
            .iter()
            .any(|f| f.code == "direct_domain_edit"));
    }
}
