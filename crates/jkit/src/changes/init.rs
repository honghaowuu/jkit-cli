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
    /// `true` when `--feature` was omitted and the slug was derived from the
    /// single change file's basename. `false` when the caller passed `--feature`.
    feature_inferred: bool,
}

pub fn run(feature: Option<&str>, files: &[String], date: Option<&str>) -> Result<()> {
    let cwd = std::env::current_dir().context("reading cwd")?;
    let report = init(&cwd, feature, files, date)?;
    print_json(&report)
}

pub fn init(
    cwd: &Path,
    feature: Option<&str>,
    files: &[String],
    date: Option<&str>,
) -> Result<InitOutput> {
    if files.is_empty() {
        anyhow::bail!("--files must list at least one change-file basename");
    }
    let (feature, feature_inferred) = match feature {
        Some(f) if !f.trim().is_empty() => (f.to_string(), false),
        Some(_) => anyhow::bail!("--feature must be non-empty"),
        None => match files.as_ref() {
            [single] => (derive_slug(single)?, true),
            _ => anyhow::bail!(
                "--feature is required when --files lists more than one change file ({} given)",
                files.len()
            ),
        },
    };
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
        feature_inferred,
    })
}

fn rel(base: &Path, p: &Path) -> String {
    p.strip_prefix(base)
        .unwrap_or(p)
        .to_string_lossy()
        .into_owned()
}

/// Derive a slug from a single change-file basename. Strips a leading
/// `YYYY-MM-DD-` date prefix (if present) and the trailing `.md` extension.
/// Errors if the resulting slug would be empty.
fn derive_slug(basename: &str) -> Result<String> {
    let stem = basename
        .strip_suffix(".md")
        .unwrap_or(basename);
    let after_date = strip_date_prefix(stem);
    let slug = after_date.trim_matches('-');
    if slug.is_empty() {
        anyhow::bail!(
            "cannot derive feature slug from '{}' — pass --feature explicitly",
            basename
        );
    }
    Ok(slug.to_string())
}

fn strip_date_prefix(s: &str) -> &str {
    // Match `YYYY-MM-DD-` at the start, then return the remainder.
    let bytes = s.as_bytes();
    if bytes.len() < 11 {
        return s;
    }
    let is_digit = |b: u8| b.is_ascii_digit();
    let dash = |b: u8| b == b'-';
    if is_digit(bytes[0])
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
        &s[11..]
    } else {
        s
    }
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
            Some("billing-bulk-invoice"),
            &["2026-04-24-bulk.md".into()],
            Some("2026-04-24"),
        )
        .unwrap();
        assert!(out.created);
        assert!(!out.feature_inferred);
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
        let first = init(tmp.path(), Some("feat"), &files, Some("2026-04-24")).unwrap();
        assert!(first.created);
        let second = init(tmp.path(), Some("feat"), &files, Some("2026-04-24")).unwrap();
        assert!(!second.created);
    }

    #[test]
    fn rejects_collision_with_different_files() {
        let tmp = tempdir().unwrap();
        init(
            tmp.path(),
            Some("feat"),
            &["a.md".into()],
            Some("2026-04-24"),
        )
        .unwrap();
        let err = init(
            tmp.path(),
            Some("feat"),
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
            Some("feat"),
            &["a.md".into()],
            Some("2026/04/24"),
        )
        .unwrap_err();
        assert!(err.to_string().contains("invalid --date"));
    }

    #[test]
    fn rejects_empty_feature() {
        let tmp = tempdir().unwrap();
        let err = init(tmp.path(), Some("  "), &["a.md".into()], Some("2026-04-24")).unwrap_err();
        assert!(err.to_string().contains("--feature"));
    }

    #[test]
    fn rejects_empty_files() {
        let tmp = tempdir().unwrap();
        let err = init(tmp.path(), Some("feat"), &[], Some("2026-04-24")).unwrap_err();
        assert!(err.to_string().contains("--files"));
    }

    #[test]
    fn infers_feature_from_single_file_with_date_prefix() {
        let tmp = tempdir().unwrap();
        let out = init(
            tmp.path(),
            None,
            &["2026-04-24-billing-bulk-invoice.md".into()],
            Some("2026-04-24"),
        )
        .unwrap();
        assert!(out.feature_inferred);
        assert!(out.run_dir.ends_with("2026-04-24-billing-bulk-invoice"));
    }

    #[test]
    fn infers_feature_from_single_file_without_date_prefix() {
        let tmp = tempdir().unwrap();
        let out = init(
            tmp.path(),
            None,
            &["bulk-invoice.md".into()],
            Some("2026-04-24"),
        )
        .unwrap();
        assert!(out.feature_inferred);
        assert!(out.run_dir.ends_with("2026-04-24-bulk-invoice"));
    }

    #[test]
    fn rejects_inference_with_multiple_files() {
        let tmp = tempdir().unwrap();
        let err = init(
            tmp.path(),
            None,
            &["a.md".into(), "b.md".into()],
            Some("2026-04-24"),
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("--feature is required"));
        assert!(msg.contains("2 given") || msg.contains("more than one"));
    }

    #[test]
    fn rejects_inference_when_basename_yields_empty_slug() {
        let tmp = tempdir().unwrap();
        let err = init(
            tmp.path(),
            None,
            &["2026-04-24-.md".into()],
            Some("2026-04-24"),
        )
        .unwrap_err();
        assert!(err.to_string().contains("cannot derive feature slug"));
    }
}
