use anyhow::{Context, Result};
use serde::Serialize;
use std::env;
use std::path::Path;

use crate::domains::yaml::{DomainEntry, DomainsFile};
use crate::envelope;
use crate::util::print_json;

#[derive(Serialize)]
pub struct DomainsAddReport {
    pub slug: String,
    pub created: bool,
    pub description: String,
    pub use_when: String,
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
    if let Some(existing) = file.get(slug) {
        let same_desc = existing.description.as_deref() == Some(description);
        let same_uw = existing.use_when.as_deref() == Some(use_when);
        if same_desc && same_uw {
            return Ok(AddOutcome::Ok(DomainsAddReport {
                slug: slug.to_string(),
                created: false,
                description: description.to_string(),
                use_when: use_when.to_string(),
            }));
        }
        return Ok(AddOutcome::Collision {
            existing_description: existing.description,
            existing_use_when: existing.use_when,
        });
    }
    file.upsert(slug, DomainEntry::new(description, use_when));
    file.save()?;
    Ok(AddOutcome::Ok(DomainsAddReport {
        slug: slug.to_string(),
        created: true,
        description: description.to_string(),
        use_when: use_when.to_string(),
    }))
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
            AddOutcome::Ok(r) => assert!(r.created),
            _ => panic!(),
        }
        // Same args → idempotent, created=false.
        match perform(tmp.path(), "billing", "invoices", "for invoicing").unwrap() {
            AddOutcome::Ok(r) => assert!(!r.created),
            _ => panic!(),
        }
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
