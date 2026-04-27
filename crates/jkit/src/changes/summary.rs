use anyhow::{Context, Result};
use chrono::Local;
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};

use crate::scenarios::gap;
use crate::util::{atomic_write, print_json};

pub struct SummaryArgs {
    pub run: PathBuf,
    pub feature: String,
    /// Domains scoped to this change; gap counts are computed only for these.
    /// `None` ⇒ scan all subdirs of `docs/domains/` (best-effort fallback).
    pub domains: Option<Vec<String>>,
    /// JUnit test source root. Default: `src/test/java/` (set by clap).
    pub test_root: PathBuf,
    pub date: Option<String>,
}

#[derive(Serialize, Debug)]
struct SummaryReport {
    summary_path: String,
    todos: Vec<&'static str>,
    gap_total: u32,
    gap_domains: u32,
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

    let domains_root = cwd.join("docs/domains");
    let domain_list = match args.domains.as_ref() {
        Some(d) => d.clone(),
        None => list_domain_subdirs(&domains_root)?,
    };
    let test_root = if args.test_root.is_absolute() {
        args.test_root.clone()
    } else {
        cwd.join(&args.test_root)
    };
    let counts = if domain_list.is_empty() {
        Vec::new()
    } else {
        gap::count_gaps_per_domain(&domains_root, &domain_list, &test_root)?
    };
    let gap_total: u32 = counts.iter().map(|(_, c)| *c as u32).sum();
    let gap_domains: u32 = counts.iter().filter(|(_, c)| *c > 0).count() as u32;

    let markdown = render_markdown(&args.feature, &date, &change_files, gap_total, gap_domains);

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
        gap_total,
        gap_domains,
    })
}

fn list_domain_subdirs(domains_root: &Path) -> Result<Vec<String>> {
    if !domains_root.is_dir() {
        return Ok(Vec::new());
    }
    let mut out: Vec<String> = fs::read_dir(domains_root)
        .with_context(|| format!("reading {}", domains_root.display()))?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    out.sort();
    Ok(out)
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

    fn write_yaml(cwd: &Path, rel: &str, contents: &str) {
        let p = cwd.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, contents).unwrap();
    }

    fn args(domains: Option<Vec<String>>) -> SummaryArgs {
        SummaryArgs {
            run: PathBuf::from(".jkit/2026-04-24-feat"),
            feature: "feat".into(),
            domains,
            test_root: PathBuf::from("src/test/java"),
            date: Some("2026-04-24".into()),
        }
    }

    #[test]
    fn writes_skeleton_with_derived_gap_counts() {
        let tmp = tempdir().unwrap();
        let run = setup_run(tmp.path(), "a.md\nb.md\n");
        write_yaml(
            tmp.path(),
            "docs/domains/billing/test-scenarios.yaml",
            "- endpoint: POST /x\n  id: a-b\n  description: foo\n- endpoint: POST /x\n  id: c-d\n  description: bar\n",
        );
        write_yaml(
            tmp.path(),
            "docs/domains/inventory/test-scenarios.yaml",
            "- endpoint: GET /y\n  id: e-f\n  description: baz\n",
        );
        let report = generate(
            tmp.path(),
            args(Some(vec!["billing".into(), "inventory".into()])),
        )
        .unwrap();
        assert_eq!(report.gap_total, 3);
        assert_eq!(report.gap_domains, 2);
        let body = fs::read_to_string(run.join("change-summary.md")).unwrap();
        assert!(body.contains("# Change Summary: feat"));
        assert!(body.contains("**Date:** 2026-04-24"));
        assert!(body.contains("**Change files:** a.md, b.md"));
        assert!(body.contains("3 unimplemented across 2 domains"));
        assert_eq!(report.todos.len(), 3);
    }

    #[test]
    fn skips_implemented_scenarios_in_count() {
        let tmp = tempdir().unwrap();
        setup_run(tmp.path(), "a.md\n");
        write_yaml(
            tmp.path(),
            "docs/domains/billing/test-scenarios.yaml",
            "- endpoint: POST /x\n  id: a-b\n  description: foo\n- endpoint: POST /x\n  id: c-d\n  description: bar\n",
        );
        write_yaml(
            tmp.path(),
            "src/test/java/BillingTest.java",
            "class BillingTest { @Test void aB() {} }",
        );
        let report = generate(tmp.path(), args(Some(vec!["billing".into()]))).unwrap();
        assert_eq!(report.gap_total, 1, "aB is implemented; only c-d remains");
        assert_eq!(report.gap_domains, 1);
    }

    #[test]
    fn omits_gap_section_when_no_gaps() {
        let tmp = tempdir().unwrap();
        setup_run(tmp.path(), "a.md\n");
        let report = generate(tmp.path(), args(Some(vec![]))).unwrap();
        assert_eq!(report.gap_total, 0);
        assert_eq!(report.gap_domains, 0);
        let body =
            fs::read_to_string(tmp.path().join(".jkit/2026-04-24-feat/change-summary.md")).unwrap();
        assert!(!body.contains("## Test Scenario Gaps"));
    }

    #[test]
    fn singular_domain_in_gap_line() {
        let tmp = tempdir().unwrap();
        setup_run(tmp.path(), "a.md\n");
        write_yaml(
            tmp.path(),
            "docs/domains/billing/test-scenarios.yaml",
            "- endpoint: POST /x\n  id: a-b\n  description: foo\n- endpoint: POST /x\n  id: c-d\n  description: bar\n- endpoint: POST /x\n  id: e-f\n  description: baz\n",
        );
        generate(tmp.path(), args(Some(vec!["billing".into()]))).unwrap();
        let body =
            fs::read_to_string(tmp.path().join(".jkit/2026-04-24-feat/change-summary.md")).unwrap();
        assert!(body.contains("3 unimplemented across 1 domain —"));
    }

    #[test]
    fn no_domains_arg_falls_back_to_scanning_all_domain_subdirs() {
        let tmp = tempdir().unwrap();
        setup_run(tmp.path(), "a.md\n");
        write_yaml(
            tmp.path(),
            "docs/domains/billing/test-scenarios.yaml",
            "- endpoint: GET /x\n  id: a-b\n  description: foo\n",
        );
        write_yaml(
            tmp.path(),
            "docs/domains/inventory/test-scenarios.yaml",
            "- endpoint: GET /y\n  id: c-d\n  description: bar\n",
        );
        let report = generate(tmp.path(), args(None)).unwrap();
        assert_eq!(report.gap_total, 2);
        assert_eq!(report.gap_domains, 2);
    }

    #[test]
    fn missing_run_dir_fails() {
        let tmp = tempdir().unwrap();
        let err = generate(
            tmp.path(),
            SummaryArgs {
                run: PathBuf::from(".jkit/missing"),
                feature: "feat".into(),
                domains: Some(vec![]),
                test_root: PathBuf::from("src/test/java"),
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
                domains: Some(vec![]),
                test_root: PathBuf::from("src/test/java"),
                date: None,
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("empty"));
    }
}
