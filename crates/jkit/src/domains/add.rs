use anyhow::{Context, Result};
use serde::Serialize;
use std::env;
use std::path::Path;

use crate::domains::yaml::{DomainEntry, DomainsFile};
use crate::envelope;
use crate::scenarios_yaml::ScenariosFile;
use crate::util::print_json;

#[derive(Serialize)]
pub struct DomainsAddReport {
    pub slug: String,
    pub created: bool,
    pub description: String,
    pub use_when: String,
    /// True when this run created the empty `domains.<slug>: []` skeleton in
    /// `docs/test-scenarios.yaml`. False when the entry was already present.
    pub scenarios_seeded: bool,
}

pub fn run(slug: &str, description: &str, use_when: &str) -> Result<()> {
    let cwd = env::current_dir().context("reading cwd")?;
    if let Err(e) = validate_slug(slug) {
        envelope::print_err_coded("invalid_slug", &e, None);
    }
    match perform(&cwd, slug, description, use_when)? {
        AddOutcome::Ok(report) => print_json(&report),
        AddOutcome::Collision { existing_description, existing_use_when } => {
            let msg = format!(
                "domain `{slug}` already exists with different fields. \
existing description: {:?}, existing use_when: {:?}. \
Update individual fields with `jkit domains set-description` / `set-use-when`.",
                existing_description, existing_use_when
            );
            envelope::print_err_coded(
                "slug_collision",
                &msg,
                Some("Run `jkit domains set-description --slug <s> --text <t>` or `set-use-when` to change one field at a time."),
            )
        }
    }
}

pub enum AddOutcome {
    Ok(DomainsAddReport),
    Collision {
        existing_description: Option<String>,
        existing_use_when: Option<String>,
    },
}

pub fn perform(
    project_root: &Path,
    slug: &str,
    description: &str,
    use_when: &str,
) -> Result<AddOutcome> {
    let mut file = DomainsFile::load(project_root)?;
    let already_present = file.get(slug);
    if let Some(existing) = &already_present {
        let same_desc = existing.description.as_deref() == Some(description);
        let same_uw = existing.use_when.as_deref() == Some(use_when);
        if !(same_desc && same_uw) {
            return Ok(AddOutcome::Collision {
                existing_description: existing.description.clone(),
                existing_use_when: existing.use_when.clone(),
            });
        }
    }
    let domains_changed = already_present.is_none();
    if domains_changed {
        file.upsert(slug, DomainEntry::new(description, use_when));
        file.save()?;
    }
    let scenarios_seeded = seed_scenarios_skeleton(project_root, slug)?;
    Ok(AddOutcome::Ok(DomainsAddReport {
        slug: slug.to_string(),
        created: domains_changed,
        description: description.to_string(),
        use_when: use_when.to_string(),
        scenarios_seeded,
    }))
}

/// Idempotent: ensure `docs/test-scenarios.yaml` has a `domains.<slug>` key
/// (empty flat list if absent). Returns `true` when this call created the
/// entry, `false` when it was already present.
fn seed_scenarios_skeleton(project_root: &Path, slug: &str) -> Result<bool> {
    let mut file = ScenariosFile::load(project_root)?;
    let already = file.slugs().iter().any(|s| s == slug);
    if already {
        return Ok(false);
    }
    file.ensure_slug_seeded(slug);
    file.save()?;
    Ok(true)
}

fn validate_slug(slug: &str) -> Result<(), String> {
    if slug.is_empty() {
        return Err("slug must not be empty".into());
    }
    if !slug
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(format!(
            "slug `{slug}` must match [a-z0-9-]+ (lowercase, digits, hyphen)"
        ));
    }
    if slug.starts_with('-') || slug.ends_with('-') {
        return Err(format!("slug `{slug}` must not start or end with `-`"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn perform_creates_then_idempotent_replays() {
        let tmp = tempdir().unwrap();
        match perform(tmp.path(), "billing", "invoices", "for invoicing").unwrap() {
            AddOutcome::Ok(r) => {
                assert!(r.created);
                assert!(r.scenarios_seeded);
            }
            _ => panic!(),
        }
        // Same args → idempotent, created=false, scenarios already seeded.
        match perform(tmp.path(), "billing", "invoices", "for invoicing").unwrap() {
            AddOutcome::Ok(r) => {
                assert!(!r.created);
                assert!(!r.scenarios_seeded);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn perform_seeds_test_scenarios_skeleton() {
        let tmp = tempdir().unwrap();
        match perform(tmp.path(), "billing", "invoices", "for invoicing").unwrap() {
            AddOutcome::Ok(r) => assert!(r.scenarios_seeded),
            _ => panic!(),
        }
        let scen = tmp.path().join("docs/test-scenarios.yaml");
        assert!(scen.exists());
        let text = std::fs::read_to_string(&scen).unwrap();
        assert!(text.contains("billing"));
    }

    #[test]
    fn perform_collision_when_fields_differ() {
        let tmp = tempdir().unwrap();
        perform(tmp.path(), "billing", "invoices", "for invoicing").unwrap();
        match perform(tmp.path(), "billing", "different", "for invoicing").unwrap() {
            AddOutcome::Collision {
                existing_description,
                ..
            } => assert_eq!(existing_description.as_deref(), Some("invoices")),
            _ => panic!("expected collision"),
        }
    }

    #[test]
    fn validate_slug_rejects_uppercase_and_edge_hyphens() {
        assert!(validate_slug("Billing").is_err());
        assert!(validate_slug("-billing").is_err());
        assert!(validate_slug("billing-").is_err());
        assert!(validate_slug("billing_2").is_err());
        assert!(validate_slug("billing-2").is_ok());
        assert!(validate_slug("a").is_ok());
    }
}
