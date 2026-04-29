//! `jkit domains doctor` — consistency checks over `docs/domains.yaml`.
//!
//! Scope-limited for the initial slice: file-shape checks (malformed YAML,
//! missing `description` / `use_when`, length warnings, unknown per-domain
//! keys). Cross-checks against `service-meta` controllers and
//! `test-scenarios.yaml` slugs land in later slices alongside the consumers
//! that need them.

use anyhow::{Context, Result};
use serde::Serialize;
use std::env;
use std::path::Path;

use crate::domains::yaml::{
    DomainsFile, LoadError, DESCRIPTION_MAX, FILE_RELATIVE, USE_WHEN_MAX,
};
use crate::util::print_json;

#[derive(Serialize)]
pub struct DoctorReport {
    pub clean: bool,
    pub findings: Vec<Finding>,
}

#[derive(Serialize, Debug)]
pub struct Finding {
    pub code: &'static str,
    pub severity: &'static str, // "issue" | "warning"
    pub message: String,
    pub remediation: String,
}

pub fn run() -> Result<()> {
    let cwd = env::current_dir().context("reading cwd")?;
    let report = diagnose(&cwd)?;
    print_json(&report)
}

pub fn diagnose(project_root: &Path) -> Result<DoctorReport> {
    let path = project_root.join(FILE_RELATIVE);
    let mut findings = Vec::new();

    if !path.exists() {
        // Absent file is informational, not an issue — fresh projects haven't
        // run `jkit domains add` yet. Surface as a warning so it's visible
        // without blocking.
        findings.push(Finding {
            code: "domains_yaml_absent",
            severity: "warning",
            message: format!("{FILE_RELATIVE} does not exist."),
            remediation: format!(
                "Run `jkit domains add --slug <s> --description <d> --use-when <w>` once per domain."
            ),
        });
        return Ok(DoctorReport {
            clean: true,
            findings,
        });
    }

    let file = match DomainsFile::load(project_root) {
        Ok(f) => f,
        Err(LoadError::Malformed { message, .. }) => {
            findings.push(Finding {
                code: "malformed_yaml",
                severity: "issue",
                message: format!("{FILE_RELATIVE} is malformed: {message}"),
                remediation: "Hand-edit the file to valid YAML; re-run `jkit domains doctor`."
                    .to_string(),
            });
            return Ok(DoctorReport {
                clean: false,
                findings,
            });
        }
        Err(other) => return Err(other.into()),
    };

    let entries = file.entries();
    if entries.is_empty() {
        findings.push(Finding {
            code: "domains_yaml_empty",
            severity: "warning",
            message: format!("{FILE_RELATIVE} contains no domain entries."),
            remediation: format!(
                "Run `jkit domains add --slug <s> --description <d> --use-when <w>` to seed it."
            ),
        });
    }

    for (slug, entry) in entries {
        match entry.description.as_deref() {
            None => findings.push(Finding {
                code: "missing_description",
                severity: "issue",
                message: format!("domain `{slug}` is missing `description`."),
                remediation: format!(
                    "Run `jkit domains set-description --slug {slug} --text <one-line description>`."
                ),
            }),
            Some(s) if s.is_empty() => findings.push(Finding {
                code: "missing_description",
                severity: "issue",
                message: format!("domain `{slug}` has empty `description`."),
                remediation: format!(
                    "Run `jkit domains set-description --slug {slug} --text <one-line description>`."
                ),
            }),
            Some(s) if s.chars().count() > DESCRIPTION_MAX => findings.push(Finding {
                code: "description_too_long",
                severity: "warning",
                message: format!(
                    "domain `{slug}` description is {} chars (max {DESCRIPTION_MAX}).",
                    s.chars().count()
                ),
                remediation: "Shorten with `jkit domains set-description`.".to_string(),
            }),
            _ => {}
        }

        match entry.use_when.as_deref() {
            None => findings.push(Finding {
                code: "missing_use_when",
                severity: "issue",
                message: format!("domain `{slug}` is missing `use_when`."),
                remediation: format!(
                    "Run `jkit domains set-use-when --slug {slug} --text <when consumers integrate>`."
                ),
            }),
            Some(s) if s.is_empty() => findings.push(Finding {
                code: "missing_use_when",
                severity: "issue",
                message: format!("domain `{slug}` has empty `use_when`."),
                remediation: format!(
                    "Run `jkit domains set-use-when --slug {slug} --text <when consumers integrate>`."
                ),
            }),
            Some(s) if s.chars().count() > USE_WHEN_MAX => findings.push(Finding {
                code: "use_when_too_long",
                severity: "warning",
                message: format!(
                    "domain `{slug}` use_when is {} chars (max {USE_WHEN_MAX}).",
                    s.chars().count()
                ),
                remediation: "Shorten with `jkit domains set-use-when`.".to_string(),
            }),
            _ => {}
        }

        for (k, _) in &entry.extras {
            findings.push(Finding {
                code: "unknown_field",
                severity: "warning",
                message: format!(
                    "domain `{slug}` has unrecognized field `{k}`. Only `description` and `use_when` are defined; future per-domain metadata needs an explicit schema change."
                ),
                remediation: "Remove the field, or open a design proposal before adding new per-domain fields."
                    .to_string(),
            });
        }
    }

    let clean = !findings.iter().any(|f| f.severity == "issue");
    Ok(DoctorReport { clean, findings })
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
    fn absent_file_is_clean_with_warning() {
        let tmp = tempdir().unwrap();
        let r = diagnose(tmp.path()).unwrap();
        assert!(r.clean);
        assert!(r.findings.iter().any(|f| f.code == "domains_yaml_absent"));
    }

    #[test]
    fn malformed_file_is_issue() {
        let tmp = tempdir().unwrap();
        write(&tmp.path().join("docs/domains.yaml"), ":\n  - bad\n");
        let r = diagnose(tmp.path()).unwrap();
        assert!(!r.clean);
        assert!(r.findings.iter().any(|f| f.code == "malformed_yaml"));
    }

    #[test]
    fn empty_domains_section_is_warning() {
        let tmp = tempdir().unwrap();
        write(&tmp.path().join("docs/domains.yaml"), "service: x\ndomains: {}\n");
        let r = diagnose(tmp.path()).unwrap();
        assert!(r.clean);
        assert!(r.findings.iter().any(|f| f.code == "domains_yaml_empty"));
    }

    #[test]
    fn missing_fields_are_issues() {
        let tmp = tempdir().unwrap();
        write(
            &tmp.path().join("docs/domains.yaml"),
            "domains:\n  billing:\n    description: hi\n",
        );
        let r = diagnose(tmp.path()).unwrap();
        assert!(!r.clean);
        assert!(r.findings.iter().any(|f| f.code == "missing_use_when"));
    }

    #[test]
    fn over_length_fields_warn() {
        let tmp = tempdir().unwrap();
        let long_desc = "x".repeat(DESCRIPTION_MAX + 1);
        let long_uw = "y".repeat(USE_WHEN_MAX + 1);
        write(
            &tmp.path().join("docs/domains.yaml"),
            &format!(
                "domains:\n  billing:\n    description: \"{long_desc}\"\n    use_when: \"{long_uw}\"\n"
            ),
        );
        let r = diagnose(tmp.path()).unwrap();
        assert!(r.clean);
        assert!(r.findings.iter().any(|f| f.code == "description_too_long"));
        assert!(r.findings.iter().any(|f| f.code == "use_when_too_long"));
    }

    #[test]
    fn unknown_field_warns() {
        let tmp = tempdir().unwrap();
        write(
            &tmp.path().join("docs/domains.yaml"),
            "domains:\n  billing:\n    description: a\n    use_when: b\n    invariants: c\n",
        );
        let r = diagnose(tmp.path()).unwrap();
        assert!(r.clean);
        assert!(r.findings.iter().any(|f| f.code == "unknown_field"));
    }
}
