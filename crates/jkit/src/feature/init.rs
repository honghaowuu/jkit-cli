use anyhow::{Context, Result};
use chrono::Local;
use serde::Serialize;
use std::collections::HashSet;
use std::path::Path;
use std::process::Command;

use crate::domains::yaml::DomainsFile;
use crate::util::print_json;

#[derive(Serialize, Debug)]
pub struct InitReport {
    pub run_path: String,
}

pub fn run(title: &str, domains: &[String]) -> Result<()> {
    let cwd = std::env::current_dir().context("reading cwd")?;
    let report = init(&cwd, title, domains)?;
    print_json(&report)
}

pub fn init(cwd: &Path, title: &str, domains: &[String]) -> Result<InitReport> {
    if title.trim().is_empty() {
        anyhow::bail!("--title must be non-empty");
    }
    let cleaned: Vec<String> = domains
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if cleaned.is_empty() {
        anyhow::bail!("--domains must list at least one slug");
    }

    let slug = slugify(title);
    if slug.is_empty() {
        anyhow::bail!(
            "--title `{}` yields an empty slug — provide alphanumeric characters",
            title
        );
    }

    let domains_file = DomainsFile::load(cwd).with_context(|| "loading docs/domains.yaml")?;
    let known: HashSet<String> = domains_file
        .entries()
        .into_iter()
        .map(|(s, _)| s)
        .collect();
    let unknown: Vec<&str> = cleaned
        .iter()
        .filter(|d| !known.contains(d.as_str()))
        .map(|s| s.as_str())
        .collect();
    if !unknown.is_empty() {
        anyhow::bail!(
            "unknown domain slug(s) not in docs/domains.yaml: {} — run `jkit domains add` first",
            unknown.join(", ")
        );
    }

    let branch = current_branch(cwd)?;
    if !branch.starts_with("feature/") {
        anyhow::bail!(
            "must be on a feature/* branch (run `jkit feature start` first); current branch: {}",
            branch
        );
    }
    let target_branch = format!("feature/{}", slug);

    let date_str = Local::now().format("%Y-%m-%d").to_string();
    let run_name = format!("{}-{}", date_str, slug);
    let run_dir = cwd.join(".jkit").join(&run_name);
    let rel_run_path = format!(".jkit/{}/", run_name);

    // Idempotent re-run: branch already renamed AND dir exists → no-op.
    if branch == target_branch && run_dir.is_dir() {
        return Ok(InitReport {
            run_path: rel_run_path,
        });
    }

    // Collision: any committed history that touches `.jkit/<date>-<slug>/`
    // means a prior run with this slug exists somewhere (merged into main,
    // or on another branch). Pick a different title.
    if git_history_touches(cwd, &format!(".jkit/{}", run_name))? {
        anyhow::bail!(
            "{} already exists in git history (any branch). Pick a different title.",
            rel_run_path
        );
    }

    std::fs::create_dir_all(&run_dir)
        .with_context(|| format!("creating {}", run_dir.display()))?;

    if branch != target_branch {
        let out = Command::new("git")
            .arg("-C")
            .arg(cwd)
            .args(["branch", "-m", &target_branch])
            .output()
            .context("running git branch -m")?;
        if !out.status.success() {
            anyhow::bail!(
                "git branch -m {} failed: {}",
                target_branch,
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
    }

    Ok(InitReport {
        run_path: rel_run_path,
    })
}

/// Lowercase, alphanumeric + hyphens. Collapses runs of non-alphanumeric
/// characters into a single `-` and trims leading/trailing dashes.
fn slugify(title: &str) -> String {
    let mut out = String::new();
    let mut last_dash = true;
    for ch in title.chars() {
        if ch.is_ascii_alphanumeric() {
            for c in ch.to_lowercase() {
                out.push(c);
            }
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
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
            "git symbolic-ref --short HEAD failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8(out.stdout)
        .context("git symbolic-ref output not UTF-8")?
        .trim()
        .to_string())
}

fn git_history_touches(cwd: &Path, path: &str) -> Result<bool> {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["log", "--all", "--oneline", "--", path])
        .output()
        .context("running git log")?;
    // On unborn HEAD or other edge cases, treat as "no history".
    if !out.status.success() {
        return Ok(false);
    }
    Ok(!out.stdout.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
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

    fn init_repo_with_domain(cwd: &Path, slug: &str) {
        git(cwd, &["init", "-q", "-b", "main"]);
        git(cwd, &["config", "user.email", "t@t"]);
        git(cwd, &["config", "user.name", "t"]);
        fs::create_dir_all(cwd.join("docs")).unwrap();
        fs::write(
            cwd.join("docs/domains.yaml"),
            format!("domains:\n  {}:\n    description: a\n    use_when: b\n", slug),
        )
        .unwrap();
        fs::write(cwd.join("README.md"), "x").unwrap();
        git(cwd, &["add", "."]);
        git(cwd, &["commit", "-q", "-m", "init"]);
    }

    #[test]
    fn slugify_basic() {
        assert_eq!(slugify("Bulk invoice"), "bulk-invoice");
        assert_eq!(slugify("Add Bulk Invoice v2!"), "add-bulk-invoice-v2");
        assert_eq!(slugify("---trim---"), "trim");
        assert_eq!(slugify("UPPER lower MIXED"), "upper-lower-mixed");
    }

    #[test]
    fn happy_path_creates_dir_and_renames_branch() {
        let tmp = tempdir().unwrap();
        init_repo_with_domain(tmp.path(), "billing");
        git(tmp.path(), &["checkout", "-q", "-b", "feature/placeholder-x"]);

        let r = init(
            tmp.path(),
            "Bulk invoice",
            &["billing".to_string()],
        )
        .unwrap();
        assert!(r.run_path.starts_with(".jkit/"));
        assert!(r.run_path.ends_with("-bulk-invoice/"));
        // dir exists
        let date = Local::now().format("%Y-%m-%d").to_string();
        let dir = tmp.path().join(format!(".jkit/{}-bulk-invoice", date));
        assert!(dir.is_dir());
        // branch renamed
        let out = Command::new("git")
            .arg("-C")
            .arg(tmp.path())
            .args(["symbolic-ref", "--short", "HEAD"])
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            "feature/bulk-invoice"
        );
    }

    #[test]
    fn rejects_unknown_domain() {
        let tmp = tempdir().unwrap();
        init_repo_with_domain(tmp.path(), "billing");
        git(tmp.path(), &["checkout", "-q", "-b", "feature/placeholder-x"]);
        let err = init(
            tmp.path(),
            "Bulk invoice",
            &["nonexistent".to_string()],
        )
        .unwrap_err();
        assert!(err.to_string().contains("unknown domain slug"));
    }

    #[test]
    fn rejects_when_not_on_feature_branch() {
        let tmp = tempdir().unwrap();
        init_repo_with_domain(tmp.path(), "billing");
        // still on main
        let err = init(
            tmp.path(),
            "Bulk invoice",
            &["billing".to_string()],
        )
        .unwrap_err();
        assert!(err.to_string().contains("feature/* branch"));
    }

    #[test]
    fn idempotent_on_already_renamed_branch() {
        let tmp = tempdir().unwrap();
        init_repo_with_domain(tmp.path(), "billing");
        git(tmp.path(), &["checkout", "-q", "-b", "feature/placeholder-x"]);
        let r1 = init(
            tmp.path(),
            "Bulk invoice",
            &["billing".to_string()],
        )
        .unwrap();
        let r2 = init(
            tmp.path(),
            "Bulk invoice",
            &["billing".to_string()],
        )
        .unwrap();
        assert_eq!(r1.run_path, r2.run_path);
    }

    #[test]
    fn rejects_collision_with_committed_history() {
        let tmp = tempdir().unwrap();
        init_repo_with_domain(tmp.path(), "billing");
        // Pre-seed the run dir on main and commit it (simulates a prior merged feature).
        let date = Local::now().format("%Y-%m-%d").to_string();
        let run_dir = tmp.path().join(format!(".jkit/{}-bulk-invoice", date));
        fs::create_dir_all(&run_dir).unwrap();
        fs::write(run_dir.join("design.md"), "old").unwrap();
        git(tmp.path(), &["add", "."]);
        git(tmp.path(), &["commit", "-q", "-m", "prior run"]);
        // Now try to init from a fresh feature branch.
        git(tmp.path(), &["checkout", "-q", "-b", "feature/placeholder-y"]);
        // Remove the locally checked-out copy so we test the history check, not "dir exists".
        fs::remove_dir_all(&run_dir).unwrap();
        git(tmp.path(), &["add", "-A"]);
        git(tmp.path(), &["commit", "-q", "-m", "remove run"]);
        let err = init(
            tmp.path(),
            "Bulk invoice",
            &["billing".to_string()],
        )
        .unwrap_err();
        assert!(err.to_string().contains("already exists in git history"));
    }
}
