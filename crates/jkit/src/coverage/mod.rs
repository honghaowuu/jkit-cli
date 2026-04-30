pub mod filter;
pub mod model;
pub mod parser;
pub mod scorer;

use anyhow::{Context, Result};
use chrono::{SecondsFormat, Utc};
use clap::Parser;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

use crate::util::{atomic_write, print_json};
use model::{CoverageSummary, FilteredMethod, ParsedClass, Report};

#[derive(Parser, Debug)]
#[command(about = "Filter and score JaCoCo XML coverage reports")]
pub struct CoverageArgs {
    /// JaCoCo XML input file
    pub input_file: String,

    /// Output JSON file (defaults to stdout)
    #[arg(long)]
    pub output: Option<String>,

    /// Minimum score threshold (methods below this are excluded)
    #[arg(long, default_value = "0.0")]
    pub min_score: f64,

    /// Pretty-print JSON output
    #[arg(long)]
    pub pretty: bool,

    /// Include a line-coverage summary alongside the filtered methods
    #[arg(long)]
    pub summary: bool,

    /// Limit output to the top-k highest-scoring methods (0 = no limit)
    #[arg(long, default_value = "5")]
    pub top_k: usize,

    /// Track missed-line totals across runs to detect plateau (writes/reads JSON state file).
    #[arg(long, value_name = "PATH")]
    pub iteration_state: Option<PathBuf>,

    /// Restrict the gap report (and plateau accounting) to methods whose
    /// fully-qualified class name contains one of the listed domains as a
    /// `.`-separated segment. Strategies:
    ///
    ///   - `domains:<comma-list>` — explicit domain slugs
    ///   - `run:<dir>` — read `<dir>/design.md` frontmatter `domains: [...]`
    ///
    /// Without `--scope`, the gap report covers every class in the JaCoCo
    /// XML. The summary (line counts / overall %) is always project-wide
    /// regardless of scope so callers see true coverage; only the gap list
    /// and plateau total are filtered.
    #[arg(long, value_name = "STRATEGY")]
    pub scope: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Default)]
struct IterationState {
    iterations: Vec<IterationEntry>,
}

#[derive(Debug, Deserialize, Serialize)]
struct IterationEntry {
    timestamp: String,
    missed_lines_total: u64,
}

/// Output envelope when `--iteration-state` is present.
#[derive(Debug, Serialize)]
struct AugmentedSummaryReport {
    #[serde(flatten)]
    base: Report,
    iteration: usize,
    missed_lines_total: u64,
    missed_lines_delta: Option<i64>,
    consecutive_no_progress: usize,
    should_stop: bool,
    /// Echo of the resolved `--scope` domains; absent when no scope was set.
    #[serde(skip_serializing_if = "Option::is_none")]
    scope_domains: Option<Vec<String>>,
}

#[derive(Debug, Serialize)]
struct AugmentedMethodsReport {
    methods: Vec<FilteredMethod>,
    iteration: usize,
    missed_lines_total: u64,
    missed_lines_delta: Option<i64>,
    consecutive_no_progress: usize,
    should_stop: bool,
    /// Echo of the resolved `--scope` domains; absent when no scope was set.
    #[serde(skip_serializing_if = "Option::is_none")]
    scope_domains: Option<Vec<String>>,
}

pub fn run(args: CoverageArgs) -> Result<()> {
    let xml = fs::read_to_string(&args.input_file)
        .with_context(|| format!("reading {}", args.input_file))?;

    let scope_domains: Option<Vec<String>> = match &args.scope {
        None => None,
        Some(s) => Some(parse_scope(s)?),
    };

    let classes = parser::parse(&xml).map_err(|e| anyhow::anyhow!("parsing JaCoCo XML: {}", e))?;
    let mut methods = compute_filtered(&classes, args.min_score);
    if let Some(domains) = &scope_domains {
        methods = filter_by_domains(methods, domains);
    }
    if args.top_k > 0 && methods.len() > args.top_k {
        methods.truncate(args.top_k);
    }

    // Build the payload as a JSON Value. The top-level array case
    // (no --summary, no --iteration-state) is wrapped under `methods` so
    // the envelope can flatten cleanly.
    let payload: serde_json::Value = if let Some(state_path) = &args.iteration_state {
        // missed_lines_total = sum of missed_lines.length across the full
        // (untruncated) filtered set, scoped to the same domain set as the
        // gap list. Plateau accounting must match scope so the loop
        // terminates when scope-only gaps are closed, not when unrelated
        // legacy code happens to be at full coverage.
        let mut full = compute_filtered(&classes, args.min_score);
        if let Some(domains) = &scope_domains {
            full = filter_by_domains(full, domains);
        }
        let total: u64 = full.iter().map(|m| m.missed_lines.len() as u64).sum();
        let (iteration, delta, consecutive_no_progress, should_stop) =
            update_iteration_state(state_path, total)?;

        if args.summary {
            // Summary deliberately stays project-wide so callers see true
            // coverage %; only the gap list + plateau total are scoped.
            let summary = summarize(&classes);
            serde_json::to_value(AugmentedSummaryReport {
                base: Report { summary, methods },
                iteration,
                missed_lines_total: total,
                missed_lines_delta: delta,
                consecutive_no_progress,
                should_stop,
                scope_domains: scope_domains.clone(),
            })?
        } else {
            serde_json::to_value(AugmentedMethodsReport {
                methods,
                iteration,
                missed_lines_total: total,
                missed_lines_delta: delta,
                consecutive_no_progress,
                should_stop,
                scope_domains: scope_domains.clone(),
            })?
        }
    } else if args.summary {
        let summary = summarize(&classes);
        if let Some(d) = scope_domains.clone() {
            serde_json::json!({
                "summary": summary,
                "methods": methods,
                "scope_domains": d,
            })
        } else {
            serde_json::to_value(Report { summary, methods })?
        }
    } else if let Some(d) = scope_domains.clone() {
        serde_json::json!({ "methods": methods, "scope_domains": d })
    } else {
        serde_json::json!({ "methods": methods })
    };

    if let Some(path) = &args.output {
        // Serialize the payload as the user asked (pretty/compact) and write
        // it to disk. The CLI itself emits a small envelope on stdout
        // pointing at the file so callers don't have to special-case stdout.
        let body = if args.pretty {
            serde_json::to_string_pretty(&payload).context("serializing JSON")?
        } else {
            serde_json::to_string(&payload).context("serializing JSON")?
        };
        fs::write(path, body).with_context(|| format!("writing {}", path))?;
        return print_json(&serde_json::json!({ "output_path": path }));
    }

    print_json(&payload)
}

/// Resolve a `--scope` argument into a list of domain slugs. Strategies:
///   - `domains:<comma-list>` — explicit domain slugs
///   - `run:<dir>` — read `<dir>/design.md` frontmatter `domains: [...]`
fn parse_scope(value: &str) -> Result<Vec<String>> {
    if let Some(rest) = value.strip_prefix("domains:") {
        let domains: Vec<String> = rest
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if domains.is_empty() {
            anyhow::bail!("--scope domains:<list> requires at least one domain");
        }
        Ok(domains)
    } else if let Some(rest) = value.strip_prefix("run:") {
        let run_dir = std::path::PathBuf::from(rest);
        let design = crate::design::read_design(&run_dir)?;
        if design.domains.is_empty() {
            anyhow::bail!(
                "{}/design.md frontmatter `domains:` is empty",
                run_dir.display()
            );
        }
        Ok(design.domains)
    } else {
        anyhow::bail!(
            "unknown --scope strategy `{}`; expected `domains:<comma-list>` or `run:<dir>`",
            value
        )
    }
}

/// Keep only methods whose dotted FQN class name contains any of the given
/// domains as a `.`-separated segment. `com.example.demo.greeting.X` matches
/// domain `greeting`; `com.example.demo.billing.Y` does not.
fn filter_by_domains(methods: Vec<FilteredMethod>, domains: &[String]) -> Vec<FilteredMethod> {
    if domains.is_empty() {
        return methods;
    }
    methods
        .into_iter()
        .filter(|m| {
            let segs: Vec<&str> = m.class.split('.').collect();
            domains.iter().any(|d| segs.iter().any(|s| *s == d))
        })
        .collect()
}

fn compute_filtered(classes: &[ParsedClass], min_score: f64) -> Vec<FilteredMethod> {
    let mut output: Vec<FilteredMethod> = Vec::new();
    for class in classes {
        for method in &class.methods {
            if filter::is_trivial(&method.name) {
                continue;
            }
            if filter::is_fully_covered(method) {
                continue;
            }
            let total = (method.line_missed + method.line_covered) as usize;
            let missed = method.missed_lines.len();
            let s = scorer::score(method.complexity, missed, total);
            if s < min_score {
                continue;
            }
            output.push(FilteredMethod {
                class: class.class_name.clone(),
                source_file: class.source_file.clone(),
                method: method.name.clone(),
                score: s,
                missed_lines: method.missed_lines.clone(),
            });
        }
    }
    output.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    output
}

fn summarize(classes: &[ParsedClass]) -> CoverageSummary {
    let mut total_covered: u32 = 0;
    let mut total_missed: u32 = 0;
    for class in classes {
        total_covered += class.line_covered;
        total_missed += class.line_missed;
    }
    let total = total_covered + total_missed;
    let pct = if total == 0 {
        100.0
    } else {
        (total_covered as f64 / total as f64) * 100.0
    };
    CoverageSummary {
        line_coverage_pct: pct,
        lines_covered: total_covered,
        lines_missed: total_missed,
    }
}

/// Reads (or creates) the state file, appends the new sample, computes plateau.
/// Returns (iteration, delta, consecutive_no_progress, should_stop).
fn update_iteration_state(
    path: &std::path::Path,
    current_total: u64,
) -> Result<(usize, Option<i64>, usize, bool)> {
    let mut state = if path.exists() {
        match fs::read_to_string(path) {
            Ok(text) => match serde_json::from_str::<IterationState>(&text) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!(
                        "warning: iteration-state file at {} is malformed ({}); starting fresh",
                        path.display(),
                        e
                    );
                    IterationState::default()
                }
            },
            Err(e) => {
                eprintln!(
                    "warning: could not read iteration-state file at {} ({}); starting fresh",
                    path.display(),
                    e
                );
                IterationState::default()
            }
        }
    } else {
        IterationState::default()
    };

    let prev_total = state.iterations.last().map(|e| e.missed_lines_total);
    let delta: Option<i64> = prev_total.map(|p| current_total as i64 - p as i64);

    state.iterations.push(IterationEntry {
        timestamp: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
        missed_lines_total: current_total,
    });

    // consecutive_no_progress = trailing iterations where delta >= 0.
    // First iteration's delta is null and counts as 0.
    let mut consecutive_no_progress = 0usize;
    let totals: Vec<u64> = state.iterations.iter().map(|e| e.missed_lines_total).collect();
    for i in (1..totals.len()).rev() {
        let d = totals[i] as i64 - totals[i - 1] as i64;
        if d >= 0 {
            consecutive_no_progress += 1;
        } else {
            break;
        }
    }
    // Two ways to stop: classic plateau (2 trailing no-progress iters), or
    // already-empty (no gaps to close — common in scoped runs where the
    // change touched code that's fully covered by its tests).
    let should_stop = consecutive_no_progress >= 2 || current_total == 0;

    let serialized = serde_json::to_vec_pretty(&state).context("serializing iteration state")?;
    atomic_write(path, &serialized)
        .with_context(|| format!("writing iteration-state file {}", path.display()))?;

    Ok((state.iterations.len(), delta, consecutive_no_progress, should_stop))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn first_iteration_emits_null_delta_and_zero_no_progress() {
        let tmp = tempdir().unwrap();
        let p = tmp.path().join("state.json");
        let (iter, delta, no_progress, stop) = update_iteration_state(&p, 24).unwrap();
        assert_eq!(iter, 1);
        assert_eq!(delta, None);
        assert_eq!(no_progress, 0);
        assert!(!stop);
    }

    #[test]
    fn first_iteration_with_zero_total_stops_immediately() {
        let tmp = tempdir().unwrap();
        let p = tmp.path().join("state.json");
        let (iter, _, _, stop) = update_iteration_state(&p, 0).unwrap();
        assert_eq!(iter, 1);
        assert!(stop, "should stop at iter 1 when nothing left to fix");
    }

    #[test]
    fn two_consecutive_no_progress_triggers_should_stop() {
        let tmp = tempdir().unwrap();
        let p = tmp.path().join("state.json");
        update_iteration_state(&p, 24).unwrap();
        update_iteration_state(&p, 18).unwrap();
        let (_, delta, no_progress, stop) = update_iteration_state(&p, 18).unwrap();
        assert_eq!(delta, Some(0));
        assert_eq!(no_progress, 1);
        assert!(!stop);
        let (_, _, no_progress, stop) = update_iteration_state(&p, 18).unwrap();
        assert_eq!(no_progress, 2);
        assert!(stop);
    }

    fn fm(class: &str, missed: usize) -> FilteredMethod {
        FilteredMethod {
            class: class.into(),
            source_file: "X.java".into(),
            method: "m".into(),
            score: 1.0,
            missed_lines: vec![1; missed],
        }
    }

    #[test]
    fn parse_scope_explicit_domains() {
        let v = parse_scope("domains:greeting,billing").unwrap();
        assert_eq!(v, vec!["greeting", "billing"]);
    }

    #[test]
    fn parse_scope_trims_whitespace_and_drops_empty() {
        let v = parse_scope("domains: greeting , , billing ").unwrap();
        assert_eq!(v, vec!["greeting", "billing"]);
    }

    #[test]
    fn parse_scope_empty_domains_errors() {
        assert!(parse_scope("domains:").is_err());
        assert!(parse_scope("domains: , ").is_err());
    }

    #[test]
    fn parse_scope_unknown_strategy_errors() {
        assert!(parse_scope("files:src/main").is_err());
        assert!(parse_scope("greeting").is_err());
    }

    #[test]
    fn parse_scope_run_reads_design_md() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("design.md");
        fs::write(
            &path,
            "---\nfeature: x\ndomains: [greeting, billing]\n---\n\n# Title\n",
        )
        .unwrap();
        let v = parse_scope(&format!("run:{}", tmp.path().display())).unwrap();
        assert_eq!(v, vec!["greeting", "billing"]);
    }

    #[test]
    fn parse_scope_run_missing_design_errors() {
        let tmp = tempdir().unwrap();
        assert!(parse_scope(&format!("run:{}", tmp.path().display())).is_err());
    }

    #[test]
    fn filter_by_domains_keeps_segment_matches() {
        let methods = vec![
            fm("com.example.demo.greeting.GreetingController", 2),
            fm("com.example.demo.billing.InvoiceController", 3),
            fm("com.example.demo.shared.Util", 1),
        ];
        let kept = filter_by_domains(methods, &["greeting".into()]);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].class, "com.example.demo.greeting.GreetingController");
    }

    #[test]
    fn filter_by_domains_empty_keeps_all() {
        let methods = vec![fm("com.example.demo.x.Y", 1)];
        let kept = filter_by_domains(methods, &[]);
        assert_eq!(kept.len(), 1);
    }

    #[test]
    fn filter_by_domains_substring_does_not_match() {
        let methods = vec![fm("com.example.demo.greetings.X", 1)];
        let kept = filter_by_domains(methods, &["greeting".into()]);
        // `greetings` (plural) is a different segment from `greeting` — no partial matches.
        assert!(kept.is_empty());
    }

    #[test]
    fn malformed_state_file_starts_fresh() {
        let tmp = tempdir().unwrap();
        let p = tmp.path().join("state.json");
        fs::write(&p, "this is not json").unwrap();
        let (iter, delta, _, _) = update_iteration_state(&p, 5).unwrap();
        assert_eq!(iter, 1);
        assert_eq!(delta, None);
    }
}
