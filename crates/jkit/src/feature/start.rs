use anyhow::{Context, Result};
use chrono::Local;
use serde::Serialize;
use std::path::Path;
use std::process::Command;

use crate::domains::doctor as domains_doctor;
use crate::util::print_json;

#[derive(Serialize, Debug)]
pub struct StartReport {
    pub branch: String,
    pub status: &'static str, // "fresh" | "resume" | "dirty" | "off_main"
    pub doctor_ok: bool,
    pub doctor_findings: Vec<DoctorFinding>,
    pub run_path: Option<String>,
}

#[derive(Serialize, Debug)]
pub struct DoctorFinding {
    pub severity: String,
    pub message: String,
    pub remediation: String,
}

pub fn run() -> Result<()> {
    let cwd = std::env::current_dir().context("reading cwd")?;
    let report = start(&cwd)?;
    print_json(&report)
}

pub fn start(cwd: &Path) -> Result<StartReport> {
    let branch = current_branch(cwd)?;
    let default = default_branch(cwd);

    let (status, branch_out, run_path) = if branch == default {
        if working_tree_dirty(cwd)? {
            ("dirty", branch.clone(), None)
        } else {
            let placeholder = placeholder_branch_name();
            checkout_new_branch(cwd, &placeholder)?;
            ("fresh", placeholder, None)
        }
    } else if let Some(suffix) = branch.strip_prefix("feature/") {
        let run_path = find_matching_run_dir(cwd, suffix);
        ("resume", branch.clone(), run_path)
    } else {
        ("off_main", branch.clone(), None)
    };

    let doctor = domains_doctor::diagnose(cwd)?;
    let doctor_ok = doctor.clean;
    let doctor_findings = doctor
        .findings
        .into_iter()
        .map(|f| DoctorFinding {
            severity: f.severity.to_string(),
            message: f.message,
            remediation: f.remediation,
        })
        .collect();

    Ok(StartReport {
        branch: branch_out,
        status,
        doctor_ok,
        doctor_findings,
        run_path,
    })
}

fn current_branch(cwd: &Path) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["symbolic-ref", "--short", "HEAD"])
        .output()
        .context("running git symbolic-ref")?;
    if !out.status.success() {
        anyhow::bail!(
            "git symbolic-ref --short HEAD failed (not a git repo, or detached HEAD?): {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8(out.stdout)
        .context("git symbolic-ref output not UTF-8")?
        .trim()
        .to_string())
}

fn default_branch(cwd: &Path) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["config", "--get", "init.defaultBranch"])
        .output()
        .ok();
    if let Some(o) = out {
        if o.status.success() {
            let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if !s.is_empty() {
                return s;
            }
        }
    }
    "main".to_string()
}

fn working_tree_dirty(cwd: &Path) -> Result<bool> {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["status", "--porcelain"])
        .output()
        .context("running git status")?;
    if !out.status.success() {
        anyhow::bail!(
            "git status --porcelain failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(!out.stdout.is_empty())
}

fn placeholder_branch_name() -> String {
    let now = Local::now();
    format!("feature/placeholder-{}", now.format("%Y-%m-%d-%H%M%S"))
}

fn checkout_new_branch(cwd: &Path, name: &str) -> Result<()> {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["checkout", "-b", name])
        .output()
        .context("running git checkout -b")?;
    if !out.status.success() {
        anyhow::bail!(
            "git checkout -b {} failed: {}",
            name,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

/// Find `.jkit/<YYYY-MM-DD>-<suffix>/` matching the branch suffix. Returns
/// `Some(rel_path)` only when exactly one matching dir exists; otherwise
/// `None` and the skill asks the user.
fn find_matching_run_dir(cwd: &Path, suffix: &str) -> Option<String> {
    let jkit = cwd.join(".jkit");
    if !jkit.is_dir() {
        return None;
    }
    let mut matches = Vec::new();
    for entry in std::fs::read_dir(&jkit).ok()? {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        if let Some(stripped) = strip_date_prefix(&name) {
            if stripped == suffix {
                matches.push(format!(".jkit/{}/", name));
            }
        }
    }
    if matches.len() == 1 {
        Some(matches.remove(0))
    } else {
        None
    }
}

fn strip_date_prefix(s: &str) -> Option<&str> {
    if s.len() < 11 {
        return None;
    }
    let bytes = s.as_bytes();
    let is_d = |b: u8| b.is_ascii_digit();
    if is_d(bytes[0])
        && is_d(bytes[1])
        && is_d(bytes[2])
        && is_d(bytes[3])
        && bytes[4] == b'-'
        && is_d(bytes[5])
        && is_d(bytes[6])
        && bytes[7] == b'-'
        && is_d(bytes[8])
        && is_d(bytes[9])
        && bytes[10] == b'-'
    {
        Some(&s[11..])
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;
    use tempfile::tempdir;

    fn git(cwd: &Path, args: &[&str]) {
        let s = Command::new("git")
            .arg("-C")
            .arg(cwd)
            .args(args)
            .status()
            .unwrap();
        assert!(s.success(), "git {:?} failed", args);
    }

    fn init_repo(cwd: &Path) {
        git(cwd, &["init", "-q", "-b", "main"]);
        git(cwd, &["config", "user.email", "t@t"]);
        git(cwd, &["config", "user.name", "t"]);
        fs::write(cwd.join("README.md"), "x").unwrap();
        git(cwd, &["add", "."]);
        git(cwd, &["commit", "-q", "-m", "init"]);
    }

    #[test]
    fn fresh_on_clean_main_creates_placeholder_and_returns_fresh() {
        let tmp = tempdir().unwrap();
        init_repo(tmp.path());
        let r = start(tmp.path()).unwrap();
        assert_eq!(r.status, "fresh");
        assert!(r.branch.starts_with("feature/placeholder-"));
        assert!(r.run_path.is_none());
    }

    #[test]
    fn dirty_main_returns_dirty() {
        let tmp = tempdir().unwrap();
        init_repo(tmp.path());
        fs::write(tmp.path().join("dirt.txt"), "uncommitted").unwrap();
        let r = start(tmp.path()).unwrap();
        assert_eq!(r.status, "dirty");
        assert_eq!(r.branch, "main");
    }

    #[test]
    fn feature_branch_returns_resume_with_matching_run_dir() {
        let tmp = tempdir().unwrap();
        init_repo(tmp.path());
        git(tmp.path(), &["checkout", "-q", "-b", "feature/bulk-invoice"]);
        fs::create_dir_all(tmp.path().join(".jkit/2026-04-30-bulk-invoice")).unwrap();
        let r = start(tmp.path()).unwrap();
        assert_eq!(r.status, "resume");
        assert_eq!(r.branch, "feature/bulk-invoice");
        assert_eq!(
            r.run_path.as_deref(),
            Some(".jkit/2026-04-30-bulk-invoice/")
        );
    }

    #[test]
    fn feature_branch_with_no_matching_dir_returns_resume_with_null_run_path() {
        let tmp = tempdir().unwrap();
        init_repo(tmp.path());
        git(tmp.path(), &["checkout", "-q", "-b", "feature/orphan"]);
        let r = start(tmp.path()).unwrap();
        assert_eq!(r.status, "resume");
        assert!(r.run_path.is_none());
    }

    #[test]
    fn other_branch_returns_off_main() {
        let tmp = tempdir().unwrap();
        init_repo(tmp.path());
        git(tmp.path(), &["checkout", "-q", "-b", "experiment"]);
        let r = start(tmp.path()).unwrap();
        assert_eq!(r.status, "off_main");
        assert_eq!(r.branch, "experiment");
    }

    #[test]
    fn doctor_findings_are_folded_in() {
        let tmp = tempdir().unwrap();
        init_repo(tmp.path());
        // Malformed domains.yaml → doctor issue.
        fs::create_dir_all(tmp.path().join("docs")).unwrap();
        fs::write(
            tmp.path().join("docs/domains.yaml"),
            ":\n  - broken\n",
        )
        .unwrap();
        git(tmp.path(), &["add", "."]);
        git(tmp.path(), &["commit", "-q", "-m", "broken"]);
        let r = start(tmp.path()).unwrap();
        assert!(!r.doctor_ok);
        assert!(r
            .doctor_findings
            .iter()
            .any(|f| f.severity == "issue"));
    }
}
