use anyhow::{Context, Result};
use serde::Serialize;
use std::env;
use std::path::Path;

use crate::domains::yaml::DomainsFile;
use crate::envelope;
use crate::util::print_json;

#[derive(Serialize)]
pub struct SetReport {
    pub slug: String,
    pub field: &'static str,
    pub text: String,
}

pub fn run_description(slug: &str, text: &str) -> Result<()> {
    let cwd = env::current_dir().context("reading cwd")?;
    match update(&cwd, slug, "description", text)? {
        Some(r) => print_json(&r),
        None => not_found(slug),
    }
}

pub fn run_use_when(slug: &str, text: &str) -> Result<()> {
    let cwd = env::current_dir().context("reading cwd")?;
    match update(&cwd, slug, "use_when", text)? {
        Some(r) => print_json(&r),
        None => not_found(slug),
    }
}

fn not_found(slug: &str) -> ! {
    envelope::print_err_coded(
        "not_found",
        &format!("domain `{slug}` not found in docs/domains.yaml"),
        Some("Add it first with `jkit domains add --slug <s> --description <d> --use-when <w>`."),
    )
}

pub fn update(
    project_root: &Path,
    slug: &str,
    field: &'static str,
    text: &str,
) -> Result<Option<SetReport>> {
    let mut file = DomainsFile::load(project_root)?;
    let result = match field {
        "description" => file.set_description(slug, text),
        "use_when" => file.set_use_when(slug, text),
        _ => unreachable!("set::update called with unsupported field {field}"),
    };
    if result.is_err() {
        return Ok(None);
    }
    file.save()?;
    Ok(Some(SetReport {
        slug: slug.to_string(),
        field,
        text: text.to_string(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domains::add::{perform as add_perform, AddOutcome};
    use tempfile::tempdir;

    #[test]
    fn update_changes_field_and_returns_report() {
        let tmp = tempdir().unwrap();
        match add_perform(tmp.path(), "billing", "invoices", "for invoicing").unwrap() {
            AddOutcome::Ok(_) => {}
            _ => panic!(),
        }
        let r = update(tmp.path(), "billing", "description", "updated").unwrap();
        assert_eq!(r.unwrap().text, "updated");
        let file = DomainsFile::load(tmp.path()).unwrap();
        assert_eq!(
            file.get("billing").unwrap().description.as_deref(),
            Some("updated")
        );
    }

    #[test]
    fn update_unknown_slug_returns_none() {
        let tmp = tempdir().unwrap();
        let r = update(tmp.path(), "nope", "description", "x").unwrap();
        assert!(r.is_none());
    }
}
