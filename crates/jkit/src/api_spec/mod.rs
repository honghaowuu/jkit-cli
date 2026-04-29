//! `jkit api-spec show <slug>` — derives an OpenAPI 3.x document for one
//! domain by running smartdoc against the project's controllers and slicing
//! the result to operations whose owning controller maps to the requested
//! slug. Read-only; the underlying smartdoc invocation snapshots and
//! restores `pom.xml` / `smart-doc.json`.
//!
//! Used by `/post-edit` for ad-hoc spec inspection and by any consumer that
//! needs the current code's API surface for one domain without reaching
//! through the full slice-per-domain machinery.

use anyhow::{Context, Result};
use clap::Subcommand;
use serde::Serialize;
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::contract::service_meta;
use crate::util::print_json;

#[derive(Subcommand, Debug)]
pub enum ApiSpecCmd {
    /// Print the OpenAPI 3.x slice for one domain.
    Show {
        /// Domain slug; must match a `controllers[].domain_slug` from
        /// `jkit contract service-meta`.
        slug: String,
        #[arg(long, default_value = "src/main/java")]
        src: PathBuf,
        #[arg(long, default_value = "pom.xml")]
        pom: PathBuf,
    },
}

pub fn run(cmd: ApiSpecCmd) -> Result<()> {
    match cmd {
        ApiSpecCmd::Show { slug, src, pom } => show(&slug, &src, &pom),
    }
}

#[derive(Serialize)]
struct ShowReport {
    slug: String,
    matched_operations: usize,
    /// The sliced OpenAPI document. Embedded as a structured value so callers
    /// can pipe through `jq` / `serde_json::from_value` without re-parsing
    /// a YAML/JSON string.
    spec: JsonValue,
}

pub fn show(slug: &str, src: &Path, pom: &Path) -> Result<()> {
    let cwd = std::env::current_dir().context("reading cwd")?;
    let src_abs = if src.is_absolute() {
        src.to_path_buf()
    } else {
        cwd.join(src)
    };
    let pom_abs = if pom.is_absolute() {
        pom.to_path_buf()
    } else {
        cwd.join(pom)
    };

    // service-meta gives us the controller→slug map (and a single source of
    // truth for `derive_domain_slug` ↔ `--domain-map` resolution if we ever
    // wire one through here).
    let meta = service_meta::compute(&pom_abs, &src_abs)
        .context("running service-meta to resolve controller→slug")?;

    // Filter to controllers belonging to this slug; surface a nicer
    // diagnostic when the slug doesn't match anything.
    let class_set: std::collections::BTreeSet<String> = meta
        .controllers
        .iter()
        .filter(|c| c.domain_slug == slug)
        .map(|c| c.class.clone())
        .collect();
    if class_set.is_empty() {
        anyhow::bail!(
            "no controllers map to slug `{slug}`. \
Run `jkit contract service-meta | jq '.controllers[].domain_slug'` to see available slugs."
        );
    }

    // (method, path) -> class so we can match smartdoc operations back to a
    // controller and decide whether they belong in the slice.
    let mut ep_to_class: HashMap<(String, String), String> = HashMap::new();
    for c in &meta.controllers {
        for m in &c.methods {
            if let (Some(h), Some(p)) = (&m.http_method, &m.path) {
                ep_to_class.insert((h.to_uppercase(), normalize_path(p)), c.class.clone());
            }
        }
    }

    let raw = crate::migrate::smartdoc::run_smartdoc(&pom_abs)?;

    let mut sliced_paths = serde_json::Map::new();
    let mut matched = 0usize;
    if let Some(paths_obj) = raw.get("paths").and_then(|p| p.as_object()) {
        for (path_str, methods_v) in paths_obj {
            let Some(methods_obj) = methods_v.as_object() else {
                continue;
            };
            let mut keep = serde_json::Map::new();
            for (method_k, op_v) in methods_obj {
                let key = (method_k.to_uppercase(), normalize_path(path_str));
                let Some(class) = ep_to_class.get(&key) else {
                    continue;
                };
                if class_set.contains(class) {
                    keep.insert(method_k.clone(), op_v.clone());
                    matched += 1;
                }
            }
            if !keep.is_empty() {
                sliced_paths.insert(path_str.clone(), JsonValue::Object(keep));
            }
        }
    }

    let mut doc = serde_json::Map::new();
    if let Some(v) = raw.get("openapi") {
        doc.insert("openapi".into(), v.clone());
    } else {
        doc.insert("openapi".into(), JsonValue::String("3.0.3".into()));
    }
    let mut info = raw
        .get("info")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({"title": slug, "version": "0.0.0"}));
    if let Some(info_obj) = info.as_object_mut() {
        info_obj.insert("title".into(), JsonValue::String(slug.to_string()));
    }
    doc.insert("info".into(), info);
    if let Some(servers) = raw.get("servers") {
        doc.insert("servers".into(), servers.clone());
    }
    doc.insert("paths".into(), JsonValue::Object(sliced_paths));
    if let Some(components) = raw.get("components") {
        // Inline all components — slice doesn't do reachability analysis,
        // so $ref paths in operations may resolve anywhere. Wasteful but
        // self-contained; matches the migrate slicer's policy.
        doc.insert("components".into(), components.clone());
    }

    let report = ShowReport {
        slug: slug.to_string(),
        matched_operations: matched,
        spec: JsonValue::Object(doc),
    };
    print_json(&report)
}

fn normalize_path(p: &str) -> String {
    let trimmed = p.trim_end_matches('/');
    if trimmed.is_empty() {
        "/".into()
    } else {
        trimmed.into()
    }
}
