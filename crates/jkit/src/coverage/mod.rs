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

use crate::util::atomic_write;
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
}

#[derive(Debug, Serialize)]
struct AugmentedMethodsReport {
    methods: Vec<FilteredMethod>,
    iteration: usize,
    missed_lines_total: u64,
    missed_lines_delta: Option<i64>,
    consecutive_no_progress: usize,
    should_stop: bool,
}

pub fn run(args: CoverageArgs) -> Result<()> {
    let xml = fs::read_to_string(&args.input_file)
        .with_context(|| format!("reading {}", args.input_file))?;

    let classes = parser::parse(&xml).map_err(|e| anyhow::anyhow!("parsing JaCoCo XML: {}", e))?;
    let mut methods = compute_filtered(&classes, args.min_score);
    if args.top_k > 0 && methods.len() > args.top_k {
        methods.truncate(args.top_k);
    }

    let json = if let Some(state_path) = &args.iteration_state {
        // missed_lines_total = sum of missed_lines.length across all post-filter methods
        // computed BEFORE top_k truncation. The PRD says "after standard v1.0 filtering",
        // and `methods` above is already truncated. Recompute over full filtered set:
        let full = compute_filtered(&classes, args.min_score);
        let total: u64 = full.iter().map(|m| m.missed_lines.len() as u64).sum();
        let (iteration, delta, consecutive_no_progress, should_stop) =
            update_iteration_state(state_path, total)?;

        if args.summary {
            let summary = summarize(&classes);
            let envelope = AugmentedSummaryReport {
                base: Report { summary, methods },
                iteration,
                missed_lines_total: total,
                missed_lines_delta: delta,
                consecutive_no_progress,
                should_stop,
            };
            serialize(&envelope, args.pretty)?
        } else {
            let envelope = AugmentedMethodsReport {
                methods,
                iteration,
                missed_lines_total: total,
                missed_lines_delta: delta,
                consecutive_no_progress,
                should_stop,
            };
            serialize(&envelope, args.pretty)?
        }
    } else if args.summary {
        let summary = summarize(&classes);
        serialize(&Report { summary, methods }, args.pretty)?
    } else {
        serialize(&methods, args.pretty)?
    };

    match &args.output {
        Some(path) => fs::write(path, &json).with_context(|| format!("writing {}", path))?,
        None => println!("{}", json),
    }
    Ok(())
}

fn serialize<T: Serialize>(value: &T, pretty: bool) -> Result<String> {
    if pretty {
        serde_json::to_string_pretty(value).context("serializing JSON")
    } else {
        serde_json::to_string(value).context("serializing JSON")
    }
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
    let should_stop = consecutive_no_progress >= 2;

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
