use anyhow::{Context, Result};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;

use crate::contract::service_meta;
use crate::util::print_json;

#[derive(Serialize)]
struct InferReport {
    candidates: Vec<DomainCandidate>,
    warnings: Vec<String>,
}

#[derive(Serialize)]
struct DomainCandidate {
    slug: String,
    controllers: Vec<ControllerSummary>,
}

#[derive(Serialize)]
struct ControllerSummary {
    class: String,
    file: String,
    method_count: usize,
}

pub fn run(src_root: &Path, pom: &Path) -> Result<()> {
    let meta = service_meta::compute(pom, src_root)
        .context("scanning controllers via service_meta::compute")?;

    let mut by_slug: BTreeMap<String, Vec<ControllerSummary>> = BTreeMap::new();
    for c in &meta.controllers {
        by_slug.entry(c.domain_slug.clone()).or_default().push(ControllerSummary {
            class: c.class.clone(),
            file: c.file.clone(),
            method_count: c.methods.len(),
        });
    }

    let report = InferReport {
        candidates: by_slug
            .into_iter()
            .map(|(slug, controllers)| DomainCandidate { slug, controllers })
            .collect(),
        warnings: meta.warnings.clone(),
    };

    print_json(&report)
}
