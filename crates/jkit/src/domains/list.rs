use anyhow::{Context, Result};
use serde::Serialize;
use std::env;

use crate::domains::yaml::DomainsFile;
use crate::util::print_json;

#[derive(Serialize)]
pub struct DomainsListReport {
    pub count: usize,
    pub domains: Vec<DomainView>,
}

#[derive(Serialize)]
pub struct DomainView {
    pub slug: String,
    pub description: Option<String>,
    pub use_when: Option<String>,
}

pub fn run() -> Result<()> {
    let cwd = env::current_dir().context("reading cwd")?;
    let report = collect(&cwd)?;
    print_json(&report)
}

pub fn collect(project_root: &std::path::Path) -> Result<DomainsListReport> {
    let file = DomainsFile::load(project_root)?;
    let domains: Vec<DomainView> = file
        .entries()
        .into_iter()
        .map(|(slug, e)| DomainView {
            slug,
            description: e.description,
            use_when: e.use_when,
        })
        .collect();
    Ok(DomainsListReport {
        count: domains.len(),
        domains,
    })
}
