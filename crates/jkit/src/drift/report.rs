use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::Path;

use crate::contract::service_meta::{self, Controller};
use crate::domain_layout::{self, ApiType, DetectedLayout};
use crate::util::print_json;

#[derive(Deserialize)]
struct DomainMap {
    #[allow(dead_code)]
    schema_version: Option<u32>,
    domains: Vec<DomainMapping>,
}

#[derive(Deserialize)]
struct DomainMapping {
    slug: String,
    controllers: Vec<String>,
}

fn load_class_to_slug(path: &Path) -> Result<HashMap<String, String>> {
    let raw =
        fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let parsed: DomainMap = serde_json::from_str(&raw)
        .with_context(|| format!("parsing {} as domain map", path.display()))?;
    let mut out = HashMap::new();
    for d in parsed.domains {
        for c in d.controllers {
            out.insert(c, d.slug.clone());
        }
    }
    Ok(out)
}

#[derive(Serialize)]
struct DriftReport {
    domains: Vec<DomainDrift>,
    warnings: Vec<String>,
}

#[derive(Serialize)]
struct DomainDrift {
    name: String,
    /// `true` when this domain has ≥2 API types and per-type subdirectories
    /// are in use on disk. Findings still appear in the flat lists below
    /// but each carries an `api_type` tag for routing.
    multi_type: bool,
    matched: usize,
    controllers_without_spec: Vec<EndpointWithSource>,
    spec_without_controllers: Vec<EndpointEntry>,
    /// `true` if at least one api-spec.yaml was found across types.
    spec_yaml_present: bool,
    /// Per-API-type breakdown of `api-spec.yaml` presence. Single-type domains
    /// emit one entry; multi-type domains emit one per type checked.
    spec_yaml_present_by_type: BTreeMap<ApiType, bool>,
}

#[derive(Serialize)]
struct EndpointEntry {
    http: String,
    path: String,
    api_type: ApiType,
}

#[derive(Serialize)]
struct EndpointWithSource {
    http: String,
    path: String,
    api_type: ApiType,
    controller: String,
    method: String,
}

pub fn run(
    domain_filter: Option<&str>,
    src_root: &Path,
    pom: &Path,
    domains_root: &Path,
    domain_map_path: Option<&Path>,
) -> Result<()> {
    let meta = service_meta::compute(pom, src_root)
        .context("scanning controllers via service_meta::compute")?;

    // class FQN -> resolved slug (from domain map if provided, else
    // controller's auto-derived slug).
    let class_to_slug: HashMap<String, String> = if let Some(p) = domain_map_path {
        load_class_to_slug(p)?
    } else {
        meta.controllers
            .iter()
            .map(|c| (c.class.clone(), c.domain_slug.clone()))
            .collect()
    };

    // slug -> { api_type -> [&Controller] }. Drives per-type drift checks.
    let mut by_slug: BTreeMap<String, BTreeMap<ApiType, Vec<&Controller>>> = BTreeMap::new();
    for c in &meta.controllers {
        let slug = class_to_slug
            .get(&c.class)
            .cloned()
            .unwrap_or_else(|| c.domain_slug.clone());
        by_slug
            .entry(slug)
            .or_default()
            .entry(c.api_type)
            .or_default()
            .push(c);
    }

    if let Some(d) = domain_filter {
        by_slug.retain(|k, _| k == d);
        if domains_root.join(d).exists() && !by_slug.contains_key(d) {
            by_slug.entry(d.to_string()).or_default();
        }
    }

    if domain_filter.is_none() && domains_root.exists() {
        if let Ok(entries) = fs::read_dir(domains_root) {
            for e in entries.flatten() {
                if e.path().is_dir() {
                    if let Some(name) = e.file_name().to_str().map(|s| s.to_string()) {
                        by_slug.entry(name).or_default();
                    }
                }
            }
        }
    }

    let mut report = DriftReport {
        domains: Vec::new(),
        warnings: Vec::new(),
    };

    for (slug, ctrls_by_type) in &by_slug {
        let layout = domain_layout::detect_layout(domains_root, slug);

        // Decide which API types to check: union of (controller types observed)
        // and (types present in the on-disk layout). Empty union (no
        // controllers, no specs, just a stale dir) → check WebApi as a default
        // probe so we still report `spec_yaml_present: false`.
        let mut types_to_check: BTreeSet<ApiType> = ctrls_by_type.keys().copied().collect();
        if let DetectedLayout::MultiType(types) = &layout {
            types_to_check.extend(types.iter().copied());
        }
        if types_to_check.is_empty() {
            types_to_check.insert(ApiType::WebApi);
        }
        let multi_type = matches!(&layout, DetectedLayout::MultiType(_));

        let mut spec_yaml_present_by_type: BTreeMap<ApiType, bool> = BTreeMap::new();
        let mut all_controllers_without_spec: Vec<EndpointWithSource> = Vec::new();
        let mut all_spec_without_controllers: Vec<EndpointEntry> = Vec::new();
        let mut total_matched = 0usize;

        for api_type in types_to_check {
            let spec_path = if multi_type {
                domains_root
                    .join(slug)
                    .join(api_type.dir_name())
                    .join("api-spec.yaml")
            } else {
                domains_root.join(slug).join("api-spec.yaml")
            };
            let spec_present = spec_path.exists();
            spec_yaml_present_by_type.insert(api_type, spec_present);

            let spec_endpoints: BTreeSet<(String, String)> = if spec_present {
                let yaml = fs::read_to_string(&spec_path)
                    .with_context(|| format!("reading {}", spec_path.display()))?;
                match parse_spec_endpoints(&yaml) {
                    Ok(eps) => eps
                        .into_iter()
                        .map(|(h, p)| (h, normalize_path(&p)))
                        .collect(),
                    Err(e) => {
                        report
                            .warnings
                            .push(format!("{}: {}", spec_path.display(), e));
                        BTreeSet::new()
                    }
                }
            } else {
                BTreeSet::new()
            };

            let empty: Vec<&Controller> = Vec::new();
            let ctrls = ctrls_by_type.get(&api_type).unwrap_or(&empty);

            let mut controller_endpoints: BTreeMap<(String, String), (String, String)> =
                BTreeMap::new();
            for c in ctrls {
                for m in &c.methods {
                    if let (Some(http), Some(path)) = (&m.http_method, &m.path) {
                        let key = (http.to_uppercase(), normalize_path(path));
                        controller_endpoints.insert(key, (c.class.clone(), m.name.clone()));
                    }
                }
            }
            let controller_set: BTreeSet<(String, String)> =
                controller_endpoints.keys().cloned().collect();

            total_matched += controller_set.intersection(&spec_endpoints).count();

            for k in controller_set.difference(&spec_endpoints) {
                let (cls, mname) = controller_endpoints.get(k).cloned().unwrap_or_default();
                all_controllers_without_spec.push(EndpointWithSource {
                    http: k.0.clone(),
                    path: k.1.clone(),
                    api_type,
                    controller: cls,
                    method: mname,
                });
            }
            for k in spec_endpoints.difference(&controller_set) {
                all_spec_without_controllers.push(EndpointEntry {
                    http: k.0.clone(),
                    path: k.1.clone(),
                    api_type,
                });
            }
        }

        let any_spec_present = spec_yaml_present_by_type.values().any(|p| *p);
        report.domains.push(DomainDrift {
            name: slug.clone(),
            multi_type,
            matched: total_matched,
            controllers_without_spec: all_controllers_without_spec,
            spec_without_controllers: all_spec_without_controllers,
            spec_yaml_present: any_spec_present,
            spec_yaml_present_by_type,
        });
    }

    print_json(&report)
}

fn parse_spec_endpoints(yaml: &str) -> Result<Vec<(String, String)>> {
    let value: serde_yaml::Value = serde_yaml::from_str(yaml).context("parsing api-spec.yaml")?;
    let mut out = Vec::new();
    let Some(paths) = value.get("paths").and_then(|p| p.as_mapping()) else {
        return Ok(out);
    };
    const HTTP_METHODS: &[&str] = &["get", "post", "put", "delete", "patch", "head", "options"];
    for (path_k, methods_v) in paths {
        let Some(path) = path_k.as_str() else { continue };
        let Some(methods) = methods_v.as_mapping() else { continue };
        for (method_k, _) in methods {
            let Some(method) = method_k.as_str() else { continue };
            let m = method.to_lowercase();
            if HTTP_METHODS.contains(&m.as_str()) {
                out.push((m.to_uppercase(), path.to_string()));
            }
        }
    }
    Ok(out)
}

fn normalize_path(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        "/".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_spec_endpoints() {
        let yaml = r#"
openapi: 3.0.3
paths:
  /users:
    get:
      summary: list
    post:
      summary: create
  /users/{id}:
    get:
      summary: get one
"#;
        let mut got = parse_spec_endpoints(yaml).unwrap();
        got.sort();
        assert_eq!(
            got,
            vec![
                ("GET".to_string(), "/users".to_string()),
                ("GET".to_string(), "/users/{id}".to_string()),
                ("POST".to_string(), "/users".to_string()),
            ]
        );
    }

    #[test]
    fn ignores_non_http_keys_under_path() {
        let yaml = r#"
paths:
  /users:
    parameters: []
    get:
      summary: list
"#;
        let got = parse_spec_endpoints(yaml).unwrap();
        assert_eq!(got, vec![("GET".to_string(), "/users".to_string())]);
    }

    #[test]
    fn normalize_strips_trailing_slash() {
        assert_eq!(normalize_path("/foo"), "/foo");
        assert_eq!(normalize_path("/foo/"), "/foo");
        assert_eq!(normalize_path("/"), "/");
        assert_eq!(normalize_path(""), "/");
    }

    #[test]
    fn handles_missing_paths_key() {
        let yaml = "openapi: 3.0.3\n";
        let got = parse_spec_endpoints(yaml).unwrap();
        assert!(got.is_empty());
    }
}
