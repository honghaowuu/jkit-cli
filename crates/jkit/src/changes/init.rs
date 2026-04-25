use anyhow::{Context, Result};
use chrono::{Local, NaiveDate};
use serde::Serialize;
use std::fs;
use std::path::Path;

use crate::util::{atomic_write, print_json};

#[derive(Serialize, Debug)]
pub struct InitOutput {
    run_dir: String,
    change_files_path: String,
    created: bool,
}

pub fn run(feature: &str, files: &[String], date: Option<&str>) -> Result<()> {
    let cwd = std::env::current_dir().context("reading cwd")?;
    let report = init(&cwd, feature, files, date)?;
    print_json(&report)
}

pub fn init(cwd: &Path, feature: &str, files: &[String], date: Option<&str>) -> Result<InitOutput> {
    if feature.trim().is_empty() {
        anyhow::bail!("--feature must be non-empty");
    }
    if files.is_empty() {
        anyhow::bail!("--files must list at least one change-file basename");
    }
    let date_str = match date {
        Some(d) => {
            NaiveDate::parse_from_str(d, "%Y-%m-%d")
                .with_context(|| format!("invalid --date '{}', expected YYYY-MM-DD", d))?;
            d.to_string()
        }
        None => Local::now().format("%Y-%m-%d").to_string(),
    };
    let run_name = format!("{}-{}", date_str, feature);
    let run_dir = cwd.join(".jkit").join(&run_name);
    let change_files_path = run_dir.join(".change-files");

    let new_content = {
        let mut s = files.join("\n");
        s.push('\n');
        s
    };

    let created = if change_files_path.is_file() {
        let existing = fs::read_to_string(&change_files_path)
            .with_context(|| format!("reading {}", change_files_path.display()))?;
        if existing == new_content {
            false
        } else {
            anyhow::bail!(
                "run dir already exists with different .change-files at {}; remove it or use a different --feature",
                run_dir.display()
            );
        }
    } else {
        fs::create_dir_all(&run_dir)
            .with_context(|| format!("creating {}", run_dir.display()))?;
        atomic_write(&change_files_path, new_content.as_bytes())?;
        true
    };

    Ok(InitOutput {
        run_dir: rel(cwd, &run_dir),
        change_files_path: rel(cwd, &change_files_path),
        created,
    })
}

fn rel(base: &Path, p: &Path) -> String {
    p.strip_prefix(base)
        .unwrap_or(p)
        .to_string_lossy()
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn creates_run_dir_and_writes_change_files() {
        let tmp = tempdir().unwrap();
        let out = init(
            tmp.path(),
            "billing-bulk-invoice",
            &["2026-04-24-bulk.md".into()],
            Some("2026-04-24"),
        )
        .unwrap();
        assert!(out.created);
        assert!(out.run_dir.ends_with("2026-04-24-billing-bulk-invoice"));
        let cf = tmp
            .path()
            .join(".jkit/2026-04-24-billing-bulk-invoice/.change-files");
        assert_eq!(fs::read_to_string(cf).unwrap(), "2026-04-24-bulk.md\n");
    }

    #[test]
    fn idempotent_on_identical_input() {
        let tmp = tempdir().unwrap();
        let files = vec!["a.md".to_string(), "b.md".to_string()];
        let first = init(tmp.path(), "feat", &files, Some("2026-04-24")).unwrap();
        assert!(first.created);
        let second = init(tmp.path(), "feat", &files, Some("2026-04-24")).unwrap();
        assert!(!second.created);
    }

    #[test]
    fn rejects_collision_with_different_files() {
        let tmp = tempdir().unwrap();
        init(
            tmp.path(),
            "feat",
            &["a.md".into()],
            Some("2026-04-24"),
        )
        .unwrap();
        let err = init(
            tmp.path(),
            "feat",
            &["b.md".into()],
            Some("2026-04-24"),
        )
        .unwrap_err();
        assert!(err.to_string().contains("different .change-files"));
    }

    #[test]
    fn rejects_invalid_date() {
        let tmp = tempdir().unwrap();
        let err = init(
            tmp.path(),
            "feat",
            &["a.md".into()],
            Some("2026/04/24"),
        )
        .unwrap_err();
        assert!(err.to_string().contains("invalid --date"));
    }

    #[test]
    fn rejects_empty_feature() {
        let tmp = tempdir().unwrap();
        let err = init(tmp.path(), "  ", &["a.md".into()], Some("2026-04-24")).unwrap_err();
        assert!(err.to_string().contains("--feature"));
    }

    #[test]
    fn rejects_empty_files() {
        let tmp = tempdir().unwrap();
        let err = init(tmp.path(), "feat", &[], Some("2026-04-24")).unwrap_err();
        assert!(err.to_string().contains("--files"));
    }
}
