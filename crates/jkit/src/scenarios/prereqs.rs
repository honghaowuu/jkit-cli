use anyhow::{Context, Result};
use serde::Serialize;
use std::path::Path;
use std::process::Command;

use crate::pom::prereqs::PomReport;
use crate::pom::{self, ProfileArg};
use crate::util::{atomic_write, print_json};

const COMPOSE_TEMPLATE: &str = include_str!("../../templates/docker-compose.test.yml");

#[derive(Serialize)]
struct PrereqsReport {
    spring_boot_version: Option<String>,
    testing_strategy: &'static str,
    runtime: Option<String>,
    pom_status: PomReport,
    missing_files: Vec<String>,
    actions_taken: Vec<String>,
    ready: bool,
    blocking_errors: Vec<String>,
}

pub fn run(apply: bool, pom_path: &Path) -> Result<()> {
    let pom_text = std::fs::read_to_string(pom_path)
        .with_context(|| format!("reading {}", pom_path.display()))?;
    let sb_version = read_spring_boot_version(&pom_text)
        .ok_or_else(|| anyhow::anyhow!("could not detect <parent><version> in pom.xml"))?;

    let strategy = pick_strategy(&sb_version);
    let profile = match strategy {
        "testcontainers" => ProfileArg::Testcontainers,
        "compose" => ProfileArg::Compose,
        other => anyhow::bail!("unexpected strategy: {}", other),
    };
    let pom_status = pom::prereqs::compute(profile, apply, pom_path)?;

    let mut runtime: Option<String> = None;
    let mut missing_files: Vec<String> = Vec::new();
    let mut actions_taken: Vec<String> = Vec::new();
    let mut blocking_errors: Vec<String> = Vec::new();

    if strategy == "compose" {
        runtime = probe_runtime();
        if runtime.is_none() {
            blocking_errors.push("no container runtime found (tried docker compose, docker-compose, podman compose)".to_string());
        }
        let compose_path = Path::new("docker-compose.test.yml");
        if !compose_path.exists() {
            if apply {
                atomic_write(compose_path, COMPOSE_TEMPLATE.as_bytes())?;
                actions_taken.push("wrote docker-compose.test.yml".to_string());
            } else {
                missing_files.push("docker-compose.test.yml".to_string());
            }
        }
    }

    let pom_ready = pom_status.ready;
    let ready = pom_ready && missing_files.is_empty() && blocking_errors.is_empty();

    let report = PrereqsReport {
        spring_boot_version: Some(sb_version),
        testing_strategy: strategy,
        runtime,
        pom_status,
        missing_files,
        actions_taken,
        ready,
        blocking_errors,
    };
    print_json(&report)
}

/// Find <parent><version>. We grep within the parent block to avoid matching the
/// project's own <version>.
fn read_spring_boot_version(pom: &str) -> Option<String> {
    let parent_start = pom.find("<parent>")?;
    let parent_end = pom[parent_start..].find("</parent>")? + parent_start;
    let block = &pom[parent_start..parent_end];
    let v_start = block.find("<version>")? + "<version>".len();
    let v_end = block[v_start..].find("</version>")? + v_start;
    Some(block[v_start..v_end].trim().to_string())
}

fn pick_strategy(sb_version: &str) -> &'static str {
    let parts: Vec<&str> = sb_version.split('.').collect();
    let major: u32 = parts.first().and_then(|s| s.parse().ok()).unwrap_or(0);
    let minor: u32 = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    if major > 3 || (major == 3 && minor >= 1) {
        "testcontainers"
    } else {
        "compose"
    }
}

fn probe_runtime() -> Option<String> {
    if Command::new("docker")
        .args(["compose", "version"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        return Some("docker compose".into());
    }
    if which::which("docker-compose").is_ok() {
        return Some("docker-compose".into());
    }
    if Command::new("podman")
        .args(["compose", "version"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        return Some("podman compose".into());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_testcontainers_for_3_1_plus() {
        assert_eq!(pick_strategy("3.1.0"), "testcontainers");
        assert_eq!(pick_strategy("3.2.5"), "testcontainers");
        assert_eq!(pick_strategy("4.0.0"), "testcontainers");
    }

    #[test]
    fn picks_compose_for_under_3_1() {
        assert_eq!(pick_strategy("3.0.13"), "compose");
        assert_eq!(pick_strategy("2.7.18"), "compose");
    }

    #[test]
    fn reads_parent_version() {
        let pom = r#"<project><parent><groupId>x</groupId><artifactId>p</artifactId><version>3.2.1</version></parent><version>1.0</version></project>"#;
        assert_eq!(read_spring_boot_version(pom).as_deref(), Some("3.2.1"));
    }
}
