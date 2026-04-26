use anyhow::{Context, Result};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::Path;
use std::process::Command;

use crate::contract::service_meta::ServiceMeta;
use crate::pom::prereqs::{self as pom_prereqs};
use crate::pom::ProfileArg;
use crate::util::atomic_write;

pub struct SmartdocResult {
    /// `slug → api-spec.yaml content`. Slugs not present in this map either had
    /// no matching smartdoc operations or were skipped due to errors — the
    /// caller falls back to regex-based output for them.
    pub per_domain_yaml: BTreeMap<String, String>,
    pub matched_operations: usize,
    pub unmatched_operations: usize,
    pub warnings: Vec<String>,
}

/// Run smartdoc against the project, slice the resulting OpenAPI doc per
/// `domain_slug` using the provided controller→slug mapping, and return one
/// YAML document per domain. The intermediate `openapi.json` is cleaned up.
///
/// Failures bubble up as `Err` so the caller can fall back to regex-based
/// rendering — never poison the migration with a half-broken spec.
pub fn slice_per_domain(
    pom: &Path,
    meta: &ServiceMeta,
    class_to_slug: &HashMap<String, String>,
) -> Result<SmartdocResult> {
    let mut warnings = Vec::new();

    pom_prereqs::compute(ProfileArg::SmartDoc, true, pom)
        .context("installing smart-doc Maven plugin via pom prereqs")?;

    let out_dir = Path::new(".jkit/migrate");
    fs::create_dir_all(out_dir)
        .with_context(|| format!("creating {}", out_dir.display()))?;
    let openapi_json_path = out_dir.join("openapi.json");
    let sd_path = Path::new("smart-doc.json");
    write_smart_doc_json(sd_path, &openapi_json_path)?;

    let output = Command::new("mvn")
        .args(["-q", "smart-doc:openapi"])
        .output()
        .context("invoking `mvn smart-doc:openapi`")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let combined = format!("{stdout}\n{stderr}");
        let lines: Vec<&str> = combined.lines().collect();
        let start = lines.len().saturating_sub(20);
        anyhow::bail!(
            "mvn smart-doc:openapi failed (last 20 lines):\n{}",
            lines[start..].join("\n")
        );
    }

    if !openapi_json_path.exists() {
        anyhow::bail!(
            "smart-doc completed but did not produce {}",
            openapi_json_path.display()
        );
    }

    let raw = fs::read_to_string(&openapi_json_path)?;
    let spec: serde_json::Value = serde_json::from_str(&raw)
        .context("parsing smart-doc's openapi.json")?;

    let info = spec.get("info").cloned().unwrap_or_else(|| {
        serde_json::json!({"title": "auto-scaffolded", "version": "0.1.0"})
    });
    let openapi_version = spec
        .get("openapi")
        .cloned()
        .unwrap_or_else(|| serde_json::Value::String("3.0.3".to_string()));
    let components = spec.get("components").cloned();
    let servers = spec.get("servers").cloned();

    let mut ep_to_class: HashMap<(String, String), String> = HashMap::new();
    for c in &meta.controllers {
        for m in &c.methods {
            if let (Some(h), Some(p)) = (&m.http_method, &m.path) {
                ep_to_class.insert((h.to_uppercase(), normalize_path(p)), c.class.clone());
            }
        }
    }

    let mut per_domain_paths: BTreeMap<String, serde_json::Map<String, serde_json::Value>> =
        BTreeMap::new();
    let mut matched: usize = 0;
    let mut unmatched: usize = 0;

    const HTTP: &[&str] = &[
        "get", "post", "put", "delete", "patch", "head", "options", "trace",
    ];

    if let Some(paths_obj) = spec.get("paths").and_then(|p| p.as_object()) {
        for (path_str, methods_v) in paths_obj {
            let Some(methods_obj) = methods_v.as_object() else {
                continue;
            };
            for (method_k, op_v) in methods_obj {
                if !HTTP.contains(&method_k.as_str()) {
                    continue;
                }
                let key = (method_k.to_uppercase(), normalize_path(path_str));
                let slug = ep_to_class
                    .get(&key)
                    .and_then(|class| class_to_slug.get(class));
                let Some(slug) = slug else {
                    unmatched += 1;
                    continue;
                };
                matched += 1;
                let domain_paths = per_domain_paths.entry(slug.clone()).or_default();
                let path_methods = domain_paths
                    .entry(path_str.clone())
                    .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
                if let Some(obj) = path_methods.as_object_mut() {
                    obj.insert(method_k.clone(), op_v.clone());
                }
            }
        }
    } else {
        warnings.push("smart-doc output had no `paths` object".to_string());
    }

    if unmatched > 0 {
        warnings.push(format!(
            "smart-doc emitted {unmatched} operations that did not match any detected controller (unmapped)"
        ));
    }

    let mut per_domain_yaml: BTreeMap<String, String> = BTreeMap::new();
    for (slug, paths_obj) in per_domain_paths {
        let mut domain_doc = serde_json::Map::new();
        domain_doc.insert("openapi".into(), openapi_version.clone());

        let mut domain_info = info.clone();
        if let Some(info_obj) = domain_info.as_object_mut() {
            info_obj.insert("title".into(), serde_json::Value::String(slug.clone()));
        }
        domain_doc.insert("info".into(), domain_info);

        if let Some(s) = &servers {
            domain_doc.insert("servers".into(), s.clone());
        }
        domain_doc.insert("paths".into(), serde_json::Value::Object(paths_obj));
        if let Some(c) = &components {
            // Inline all components into each domain spec — the slicer doesn't
            // do reachability analysis, so $ref paths in operations may point
            // at any schema. Wasteful but self-contained.
            domain_doc.insert("components".into(), c.clone());
        }

        let value = serde_json::Value::Object(domain_doc);
        let yaml = serde_yaml::to_string(&value)
            .context("converting per-domain OpenAPI doc to YAML")?;
        per_domain_yaml.insert(slug, yaml);
    }

    let _ = fs::remove_file(&openapi_json_path);

    Ok(SmartdocResult {
        per_domain_yaml,
        matched_operations: matched,
        unmatched_operations: unmatched,
        warnings,
    })
}

fn normalize_path(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        "/".to_string()
    } else {
        trimmed.to_string()
    }
}

fn write_smart_doc_json(path: &Path, openapi_json_path: &Path) -> Result<()> {
    let parent = openapi_json_path
        .parent()
        .unwrap_or(Path::new("."))
        .display()
        .to_string();
    let new_value = serde_json::json!({
        "outPath": parent,
        "openApiAllInOne": true,
        "sourceCodePaths": [{"path": "src/main/java", "ignore": ""}],
        "serverUrl": "http://localhost",
        "isStrict": false,
        "allInOne": true,
        "createDebugPage": false,
        "packageFilters": "",
        "showAuthor": false
    });

    if !path.exists() {
        atomic_write(path, serde_json::to_string_pretty(&new_value)?.as_bytes())?;
        return Ok(());
    }

    // Merge: preserve user customizations, overwrite the three fields the
    // slicer cares about (outPath, openApiAllInOne, sourceCodePaths).
    let existing_text = fs::read_to_string(path)?;
    let mut existing: serde_json::Value = serde_json::from_str(&existing_text)
        .context("existing smart-doc.json is malformed JSON")?;
    if let (Some(eo), Some(no)) = (existing.as_object_mut(), new_value.as_object()) {
        for key in ["outPath", "openApiAllInOne", "sourceCodePaths"] {
            if let Some(v) = no.get(key) {
                eo.insert(key.to_string(), v.clone());
            }
        }
    }
    let merged = serde_json::to_string_pretty(&existing)?;
    atomic_write(path, merged.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_strips_trailing_slash() {
        assert_eq!(normalize_path("/foo"), "/foo");
        assert_eq!(normalize_path("/foo/"), "/foo");
        assert_eq!(normalize_path("/"), "/");
        assert_eq!(normalize_path(""), "/");
    }
}
