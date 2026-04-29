//! `jkit domain-context <slug>` — assembles a compact JSON view of one
//! domain from the artifacts the binary owns.
//!
//! ## Slice scope
//!
//! Returns the pieces we currently have:
//!   - `slug`, `description`, `use_when` from `docs/domains.yaml`
//!   - `scenarios.{total, implemented, gaps[]}` from `docs/test-scenarios.yaml`
//!     + JUnit method scan
//!
//! Future slices add `controllers[]`, `services[]`, `entities[]`,
//! `published_contract` (need smartdoc, JPA scan, call-graph, contract-repo
//! resolution). Those fields are absent from the output until then —
//! consumers should treat their absence as "not yet computed," not
//! "empty / not applicable."

use anyhow::{Context, Result};
use serde::Serialize;
use std::path::{Path, PathBuf};

use crate::domains::yaml::DomainsFile;
use crate::envelope;
use crate::scenarios::gap;
use crate::scenarios_yaml::ScenariosFile;
use crate::util::print_json;

#[derive(clap::Args, Debug)]
pub struct DomainContextArgs {
    /// Domain slug (must match a key under `domains:` in `docs/domains.yaml`).
    pub slug: String,
    /// Verbosity hint: `plan` (full), `review` (medium), `clarify` (minimal).
    /// Currently informational — all values produce the same output until
    /// the larger context fields land.
    #[arg(long = "for")]
    pub for_: Option<String>,
    /// JUnit test source root used for scenario gap detection.
    #[arg(long, default_value = "src/test/java/")]
    pub test_root: PathBuf,
}

#[derive(Serialize)]
pub struct DomainContextReport {
    pub slug: String,
    pub description: Option<String>,
    pub use_when: Option<String>,
    pub scenarios: ScenariosSummary,
    pub note: &'static str,
}

#[derive(Serialize)]
pub struct ScenariosSummary {
    pub total: usize,
    pub implemented: usize,
    /// Unimplemented scenario ids (camel-cased method name absent from
    /// `src/test/java/`).
    pub gaps: Vec<GapSummary>,
}

#[derive(Serialize)]
pub struct GapSummary {
    pub id: String,
    pub endpoint: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_type: Option<String>,
}

pub fn run(args: DomainContextArgs) -> Result<()> {
    let cwd = std::env::current_dir().context("reading cwd")?;
    let domains = DomainsFile::load(&cwd)?;
    let entry = match domains.get(&args.slug) {
        Some(e) => e,
        None => {
            envelope::print_err_coded(
                "not_found",
                &format!(
                    "domain `{}` is not declared in docs/domains.yaml",
                    args.slug
                ),
                Some(
                    "Add it first with `jkit domains add --slug <s> --description <d> --use-when <w>`.",
                ),
            );
        }
    };

    let test_root = if args.test_root.is_absolute() {
        args.test_root.clone()
    } else {
        cwd.join(&args.test_root)
    };
    let scenarios = compute_scenarios_summary(&cwd, &args.slug, &test_root)?;

    let report = DomainContextReport {
        slug: args.slug,
        description: entry.description,
        use_when: entry.use_when,
        scenarios,
        note: "controllers, services, entities, published_contract — not yet assembled by this binary",
    };
    print_json(&report)
}

fn compute_scenarios_summary(
    project_root: &Path,
    slug: &str,
    test_root: &Path,
) -> Result<ScenariosSummary> {
    let scenarios = ScenariosFile::load(project_root)?;
    let section = match scenarios.section(slug)? {
        Some(s) => s,
        None => {
            return Ok(ScenariosSummary {
                total: 0,
                implemented: 0,
                gaps: Vec::new(),
            });
        }
    };
    let implemented = gap::implemented_method_names(test_root)?;

    let mut total = 0usize;
    let mut implemented_count = 0usize;
    let mut gaps: Vec<GapSummary> = Vec::new();
    for (api_type, entry) in section.iter_with_type() {
        total += 1;
        let camel = gap::id_to_camel(&entry.id);
        if implemented.contains(&camel) {
            implemented_count += 1;
        } else {
            gaps.push(GapSummary {
                id: entry.id.clone(),
                endpoint: entry.endpoint.clone(),
                description: entry.description.clone(),
                api_type: api_type.map(str::to_string),
            });
        }
    }
    Ok(ScenariosSummary {
        total,
        implemented: implemented_count,
        gaps,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn write(p: &Path, s: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, s).unwrap();
    }

    #[test]
    fn summary_counts_implemented_vs_gaps() {
        let tmp = tempdir().unwrap();
        write(
            &tmp.path().join("docs/test-scenarios.yaml"),
            r#"domains:
  billing:
    - endpoint: GET /a
      id: alpha-bravo
      description: ok
    - endpoint: GET /b
      id: charlie-delta
      description: ok
"#,
        );
        write(
            &tmp.path().join("src/test/java/FooTest.java"),
            "class FooTest { @Test void alphaBravo() {} }",
        );
        let s = compute_scenarios_summary(
            tmp.path(),
            "billing",
            &tmp.path().join("src/test/java"),
        )
        .unwrap();
        assert_eq!(s.total, 2);
        assert_eq!(s.implemented, 1);
        assert_eq!(s.gaps.len(), 1);
        assert_eq!(s.gaps[0].id, "charlie-delta");
    }

    #[test]
    fn missing_slug_in_scenarios_yields_empty_summary() {
        let tmp = tempdir().unwrap();
        let s = compute_scenarios_summary(
            tmp.path(),
            "billing",
            &tmp.path().join("src/test/java"),
        )
        .unwrap();
        assert_eq!(s.total, 0);
        assert_eq!(s.implemented, 0);
        assert!(s.gaps.is_empty());
    }
}
