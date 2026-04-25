use anyhow::{Context, Result};
use chrono::Local;
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};

use crate::util::{atomic_write, print_json};

pub struct SummaryArgs {
    pub run: PathBuf,
    pub feature: String,
    pub gap_total: u32,
    pub gap_domains: u32,
    pub date: Option<String>,
}

#[derive(Serialize, Debug)]
struct SummaryReport {
    summary_path: String,
    todos: Vec<&'static str>,
}

pub fn run(args: SummaryArgs) -> Result<()> {
    let cwd = std::env::current_dir().context("reading cwd")?;
    let report = generate(&cwd, args)?;
    print_json(&report)
}

fn generate(cwd: &Path, args: SummaryArgs) -> Result<SummaryReport> {
    let run_dir = if args.run.is_absolute() {
        args.run.clone()
    } else {
        cwd.join(&args.run)
    };
    if !run_dir.is_dir() {
        anyhow::bail!("run dir not found: {}", run_dir.display());
    }
    let change_files_path = run_dir.join(".change-files");
    if !change_files_path.is_file() {
        anyhow::bail!(".change-files missing in {}", run_dir.display());
    }
    let change_files: Vec<String> = fs::read_to_string(&change_files_path)
        .with_context(|| format!("reading {}", change_files_path.display()))?
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect();
    if change_files.is_empty() {
        anyhow::bail!(".change-files in {} is empty", run_dir.display());
    }
    let date = args
        .date
        .clone()
        .unwrap_or_else(|| Local::now().format("%Y-%m-%d").to_string());

    let markdown = render_markdown(
        &args.feature,
        &date,
        &change_files,
        args.gap_total,
        args.gap_domains,
    );

    let summary_path = run_dir.join("change-summary.md");
    atomic_write(&summary_path, markdown.as_bytes())?;

    Ok(SummaryReport {
        summary_path: summary_path
            .strip_prefix(cwd)
            .unwrap_or(&summary_path)
            .to_string_lossy()
            .into_owned(),
        todos: vec![
            "Domains Changed (one row per affected domain)",
            "Schema Change Required (Yes/No + one-line summary if Yes)",
            "Assumptions (or remove the section if no defaults were silently picked)",
        ],
    })
}

fn render_markdown(
    feature: &str,
    date: &str,
    change_files: &[String],
    gap_total: u32,
    gap_domains: u32,
) -> String {
    let mut out = String::new();
    out.push_str(&format!("# Change Summary: {}\n\n", feature));
    out.push_str(&format!("**Date:** {}\n", date));
    out.push_str(&format!("**Change files:** {}\n\n", change_files.join(", ")));
    out.push_str("## Domains Changed\n\n");
    out.push_str("<!-- TODO: one row per affected domain. Cells are short noun phrases (entity / endpoint / field changes); use — for empty cells. -->\n");
    out.push_str("| Domain | Added | Modified | Removed |\n");
    out.push_str("|--------|-------|----------|---------|\n\n");
    out.push_str("## Schema Change Required\n\n");
    out.push_str("<!-- TODO: replace with `Yes` + one-line description, or `No`. -->\n\n");
    if gap_total > 0 {
        out.push_str("## Test Scenario Gaps\n\n");
        let plural = if gap_domains == 1 { "" } else { "s" };
        out.push_str(&format!(
            "{} unimplemented across {} domain{} — run `jkit scenarios gap <domain>` for the list.\n\n",
            gap_total, gap_domains, plural
        ));
    }
    out.push_str("## Assumptions\n\n");
    out.push_str("<!-- TODO: list defaults silently picked during clarification, or delete this section if none. -->\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn setup_run(cwd: &Path, files: &str) -> PathBuf {
        let run = cwd.join(".jkit/2026-04-24-feat");
        fs::create_dir_all(&run).unwrap();
        fs::write(run.join(".change-files"), files).unwrap();
        run
    }

    #[test]
    fn writes_skeleton_with_deterministic_fields() {
        let tmp = tempdir().unwrap();
        let run = setup_run(tmp.path(), "a.md\nb.md\n");
        let report = generate(
            tmp.path(),
            SummaryArgs {
                run: PathBuf::from(".jkit/2026-04-24-feat"),
                feature: "feat".into(),
                gap_total: 5,
                gap_domains: 2,
                date: Some("2026-04-24".into()),
            },
        )
        .unwrap();
        let body = fs::read_to_string(run.join("change-summary.md")).unwrap();
        assert!(body.contains("# Change Summary: feat"));
        assert!(body.contains("**Date:** 2026-04-24"));
        assert!(body.contains("**Change files:** a.md, b.md"));
        assert!(body.contains("5 unimplemented across 2 domains"));
        assert!(body.contains("<!-- TODO:"));
        assert_eq!(report.todos.len(), 3);
    }

    #[test]
    fn omits_gap_section_when_zero() {
        let tmp = tempdir().unwrap();
        setup_run(tmp.path(), "a.md\n");
        generate(
            tmp.path(),
            SummaryArgs {
                run: PathBuf::from(".jkit/2026-04-24-feat"),
                feature: "feat".into(),
                gap_total: 0,
                gap_domains: 0,
                date: Some("2026-04-24".into()),
            },
        )
        .unwrap();
        let body =
            fs::read_to_string(tmp.path().join(".jkit/2026-04-24-feat/change-summary.md")).unwrap();
        assert!(!body.contains("## Test Scenario Gaps"));
    }

    #[test]
    fn singular_domain_in_gap_line() {
        let tmp = tempdir().unwrap();
        setup_run(tmp.path(), "a.md\n");
        generate(
            tmp.path(),
            SummaryArgs {
                run: PathBuf::from(".jkit/2026-04-24-feat"),
                feature: "feat".into(),
                gap_total: 3,
                gap_domains: 1,
                date: Some("2026-04-24".into()),
            },
        )
        .unwrap();
        let body =
            fs::read_to_string(tmp.path().join(".jkit/2026-04-24-feat/change-summary.md")).unwrap();
        assert!(body.contains("3 unimplemented across 1 domain —"));
    }

    #[test]
    fn missing_run_dir_fails() {
        let tmp = tempdir().unwrap();
        let err = generate(
            tmp.path(),
            SummaryArgs {
                run: PathBuf::from(".jkit/missing"),
                feature: "feat".into(),
                gap_total: 0,
                gap_domains: 0,
                date: None,
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("run dir not found"));
    }

    #[test]
    fn empty_change_files_fails() {
        let tmp = tempdir().unwrap();
        setup_run(tmp.path(), "");
        let err = generate(
            tmp.path(),
            SummaryArgs {
                run: PathBuf::from(".jkit/2026-04-24-feat"),
                feature: "feat".into(),
                gap_total: 0,
                gap_domains: 0,
                date: None,
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("empty"));
    }
}
