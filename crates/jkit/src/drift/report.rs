use anyhow::{Context, Result};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use crate::contract::service_meta::{self, Controller};
use crate::util::print_json;

#[derive(Serialize)]
struct DriftReport {
    domains: Vec<DomainDrift>,
    warnings: Vec<String>,
}

#[derive(Serialize)]
struct DomainDrift {
    name: String,
    matched: usize,
    controllers_without_spec: Vec<EndpointWithSource>,
    spec_without_controllers: Vec<EndpointEntry>,
    spec_yaml_present: bool,
}

#[derive(Serialize)]
struct EndpointEntry {
    http: String,
    path: String,
}

#[derive(Serialize)]
struct EndpointWithSource {
    http: String,
    path: String,
    controller: String,
    method: String,
}

pub fn run(
    domain_filter: Option<&str>,
    src_root: &Path,
    pom: &Path,
    domains_root: &Path,
) -> Result<()> {
    let meta = service_meta::compute(pom, src_root)
        .context("scanning controllers via service_meta::compute")?;

    let mut by_slug: BTreeMap<String, Vec<&Controller>> = BTreeMap::new();
    for c in &meta.controllers {
        by_slug.entry(c.domain_slug.clone()).or_default().push(c);
    }

    if let Some(d) = domain_filter {
        by_slug.retain(|k, _| k == d);
        if domains_root.join(d).exists() && !by_slug.contains_key(d) {
            // Domain dir exists with no controllers — still report spec-only drift.
            by_slug.entry(d.to_string()).or_default();
        }
    }

    // Also include domains that have an api-spec.yaml but no controllers.
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

    for (slug, controllers) in &by_slug {
        let spec_path = domains_root.join(slug).join("api-spec.yaml");
        let spec_present = spec_path.exists();
        let spec_endpoints: BTreeSet<(String, String)> = if spec_present {
            let yaml = fs::read_to_string(&spec_path)
                .with_context(|| format!("reading {}", spec_path.display()))?;
            match parse_spec_endpoints(&yaml) {
                Ok(eps) => eps.into_iter().collect(),
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

        let mut controller_endpoints: BTreeMap<(String, String), (String, String)> =
            BTreeMap::new();
        for c in controllers {
            for m in &c.methods {
                if let (Some(http), Some(path)) = (&m.http_method, &m.path) {
                    let key = (http.to_uppercase(), normalize_path(path));
                    controller_endpoints.insert(key, (c.class.clone(), m.name.clone()));
                }
            }
        }
        let controller_set: BTreeSet<(String, String)> =
            controller_endpoints.keys().cloned().collect();

        let normalized_spec: BTreeSet<(String, String)> = spec_endpoints
            .into_iter()
            .map(|(h, p)| (h, normalize_path(&p)))
            .collect();

        let matched: usize = controller_set.intersection(&normalized_spec).count();

        let controllers_without_spec: Vec<EndpointWithSource> = controller_set
            .difference(&normalized_spec)
            .map(|k| {
                let (cls, mname) = controller_endpoints
                    .get(k)
                    .cloned()
                    .unwrap_or_default();
                EndpointWithSource {
                    http: k.0.clone(),
                    path: k.1.clone(),
                    controller: cls,
                    method: mname,
                }
            })
            .collect();

        let spec_without_controllers: Vec<EndpointEntry> = normalized_spec
            .difference(&controller_set)
            .map(|k| EndpointEntry {
                http: k.0.clone(),
                path: k.1.clone(),
            })
            .collect();

        report.domains.push(DomainDrift {
            name: slug.clone(),
            matched,
            controllers_without_spec,
            spec_without_controllers,
            spec_yaml_present: spec_present,
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
