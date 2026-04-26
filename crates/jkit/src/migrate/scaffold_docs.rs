use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::Path;

use crate::contract::service_meta::{self, Controller, MethodMeta};
use crate::domain_layout::{self, ApiType};
use crate::migrate::{call_graph, smartdoc};
use crate::util::{atomic_write, print_json};

#[derive(Serialize)]
struct ScaffoldReport {
    domains: Vec<DomainReport>,
    warnings: Vec<String>,
}

#[derive(Serialize)]
struct DomainReport {
    name: String,
    dir: String,
    /// `true` when this domain has ≥2 API types and files live in
    /// `<root>/<api-type>/...` subdirectories. `false` when the single
    /// detected type stays flat at the domain root.
    multi_type: bool,
    /// Per (slug, api_type) bucket. For single-type domains, exactly one
    /// entry whose `api_type` reflects the detected type.
    buckets: Vec<BucketReport>,
    /// Files written at the slug root (same regardless of multi-type).
    shared_files_created: Vec<String>,
    shared_files_existing: Vec<String>,
}

#[derive(Serialize)]
struct BucketReport {
    api_type: ApiType,
    dir: String,
    files_created: Vec<String>,
    files_existing: Vec<String>,
    files_overwritten: Vec<String>,
    controllers: Vec<String>,
    method_count: usize,
    api_spec_source: &'static str,
    impl_logic_source: &'static str,
}

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

pub fn run(
    src_root: &Path,
    pom: &Path,
    out_dir: &Path,
    domain_map_path: Option<&Path>,
    use_smartdoc: bool,
    use_call_graph: bool,
) -> Result<()> {
    let meta = service_meta::compute(pom, src_root)
        .context("scanning controllers via service_meta::compute")?;

    let mut warnings = meta.warnings.clone();

    let class_to_slug: HashMap<String, String> = if let Some(p) = domain_map_path {
        load_domain_map(p, &meta.controllers, &mut warnings)?
    } else {
        meta.controllers
            .iter()
            .map(|c| (c.class.clone(), c.domain_slug.clone()))
            .collect()
    };

    // class_fqn -> (slug, api_type). Drives every per-bucket lookup downstream.
    let class_to_slug_type: HashMap<String, (String, ApiType)> = meta
        .controllers
        .iter()
        .filter_map(|c| {
            class_to_slug
                .get(&c.class)
                .map(|slug| (c.class.clone(), (slug.clone(), c.api_type)))
        })
        .collect();

    // slug -> { api_type -> [&Controller] } — drives multi-type vs flat layout.
    let mut by_slug_type: BTreeMap<String, BTreeMap<ApiType, Vec<&Controller>>> = BTreeMap::new();
    for c in &meta.controllers {
        if class_to_slug.get(&c.class).is_some() {
            let slug = &class_to_slug[&c.class];
            by_slug_type
                .entry(slug.clone())
                .or_default()
                .entry(c.api_type)
                .or_default()
                .push(c);
        }
    }

    let smartdoc_yaml: BTreeMap<(String, ApiType), String> = if use_smartdoc {
        match smartdoc::slice_per_domain(pom, &meta, &class_to_slug_type) {
            Ok(result) => {
                warnings.extend(result.warnings);
                if result.matched_operations == 0 && result.unmatched_operations == 0 {
                    warnings.push(
                        "smart-doc produced no recognizable operations; falling back to regex output for all domains".to_string(),
                    );
                }
                result.per_domain_yaml
            }
            Err(e) => {
                warnings.push(format!(
                    "smart-doc failed; falling back to regex output for all domains: {e}"
                ));
                BTreeMap::new()
            }
        }
    } else {
        BTreeMap::new()
    };

    let call_graph_by_bucket: HashMap<(String, ApiType), Vec<call_graph::ControllerEntry>> =
        if use_call_graph {
            match call_graph::build(src_root, &meta.controllers, &class_to_slug_type) {
                Ok(map) => {
                    if map.is_empty() {
                        warnings.push(
                            "call-graph: no controllers parsed via tree-sitter; falling back to skeleton impl-logic for all domains".to_string(),
                        );
                    }
                    map
                }
                Err(e) => {
                    warnings.push(format!(
                        "call-graph: tree-sitter parse failed; falling back to skeleton impl-logic for all domains: {e}"
                    ));
                    HashMap::new()
                }
            }
        } else {
            HashMap::new()
        };

    let mut report = ScaffoldReport {
        domains: Vec::new(),
        warnings,
    };

    for (slug, by_type) in &by_slug_type {
        let api_types: Vec<ApiType> = by_type.keys().copied().collect();
        let paths = domain_layout::paths_for(out_dir, slug, &api_types);

        // Ensure slug root exists (for shared files even when multi-type).
        fs::create_dir_all(&paths.root)
            .with_context(|| format!("creating {}", paths.root.display()))?;

        // Shared files (domain-model.md, conflicts.md) at the slug root.
        // conflicts.md is written by `jkit conflicts report` separately —
        // scaffold-docs only handles domain-model.md here.
        let mut shared_created = Vec::new();
        let mut shared_existing = Vec::new();
        if paths.domain_model.exists() {
            shared_existing.push("domain-model.md".to_string());
        } else {
            atomic_write(&paths.domain_model, render_domain_model(slug).as_bytes())
                .with_context(|| format!("writing {}", paths.domain_model.display()))?;
            shared_created.push("domain-model.md".to_string());
        }

        let mut buckets: Vec<BucketReport> = Vec::new();
        for (api_type, controllers) in by_type {
            let per_type_paths = &paths.per_type[api_type];
            let bucket_dir = paths.dir_for(*api_type);
            fs::create_dir_all(&bucket_dir)
                .with_context(|| format!("creating {}", bucket_dir.display()))?;

            let mut created = Vec::new();
            let mut existing = Vec::new();
            let mut overwritten = Vec::new();
            let mut api_spec_source = "regex";
            let mut impl_logic_source = "regex";

            // api-spec.yaml: smartdoc overwrites when present, else skeleton (skip-if-exists)
            if let Some(yaml) = smartdoc_yaml.get(&(slug.clone(), *api_type)) {
                let was_existing = per_type_paths.api_spec.exists();
                atomic_write(&per_type_paths.api_spec, yaml.as_bytes())
                    .with_context(|| format!("writing {}", per_type_paths.api_spec.display()))?;
                if was_existing {
                    overwritten.push("api-spec.yaml".to_string());
                } else {
                    created.push("api-spec.yaml".to_string());
                }
                api_spec_source = "smartdoc";
            } else if per_type_paths.api_spec.exists() {
                existing.push("api-spec.yaml".to_string());
            } else {
                let body = render_api_spec(slug, controllers);
                atomic_write(&per_type_paths.api_spec, body.as_bytes())?;
                created.push("api-spec.yaml".to_string());
            }

            // api-implement-logic.md: call-graph overwrites when present, else skeleton
            if let Some(entries) = call_graph_by_bucket.get(&(slug.clone(), *api_type)) {
                let body = call_graph::render(slug, entries);
                let was_existing = per_type_paths.implement_logic.exists();
                atomic_write(&per_type_paths.implement_logic, body.as_bytes())?;
                if was_existing {
                    overwritten.push("api-implement-logic.md".to_string());
                } else {
                    created.push("api-implement-logic.md".to_string());
                }
                impl_logic_source = "call-graph";
            } else if per_type_paths.implement_logic.exists() {
                existing.push("api-implement-logic.md".to_string());
            } else {
                let body = render_impl_logic(slug, controllers);
                atomic_write(&per_type_paths.implement_logic, body.as_bytes())?;
                created.push("api-implement-logic.md".to_string());
            }

            // test-scenarios.yaml: skeleton, skip-if-exists
            if per_type_paths.scenarios.exists() {
                existing.push("test-scenarios.yaml".to_string());
            } else {
                let body = render_scenarios(slug, controllers);
                atomic_write(&per_type_paths.scenarios, body.as_bytes())?;
                created.push("test-scenarios.yaml".to_string());
            }

            let method_count: usize = controllers.iter().map(|c| c.methods.len()).sum();
            buckets.push(BucketReport {
                api_type: *api_type,
                dir: bucket_dir.display().to_string(),
                files_created: created,
                files_existing: existing,
                files_overwritten: overwritten,
                controllers: controllers.iter().map(|c| c.class.clone()).collect(),
                method_count,
                api_spec_source,
                impl_logic_source,
            });
        }

        report.domains.push(DomainReport {
            name: slug.clone(),
            dir: paths.root.display().to_string(),
            multi_type: paths.multi_type,
            buckets,
            shared_files_created: shared_created,
            shared_files_existing: shared_existing,
        });
    }

    print_json(&report)
}

fn render_api_spec(slug: &str, controllers: &[&Controller]) -> String {
    let mut s = String::new();
    s.push_str("openapi: 3.0.3\n");
    s.push_str("info:\n");
    s.push_str(&format!("  title: {slug}\n"));
    s.push_str("  version: \"0.1.0\"\n");
    s.push_str("  description: |\n");
    s.push_str("    Auto-scaffolded by `jkit migrate scaffold-docs` from existing controllers.\n");
    s.push_str("    Refine via /spec-delta — this is a starter skeleton, not authoritative.\n");

    let mut by_path: BTreeMap<String, BTreeMap<String, &MethodMeta>> = BTreeMap::new();
    for c in controllers {
        for m in &c.methods {
            if let (Some(http), Some(p)) = (&m.http_method, &m.path) {
                let p = if p.is_empty() {
                    "/".to_string()
                } else if !p.starts_with('/') {
                    format!("/{p}")
                } else {
                    p.clone()
                };
                by_path
                    .entry(p)
                    .or_default()
                    .insert(http.to_lowercase(), m);
            }
        }
    }

    if by_path.is_empty() {
        s.push_str("paths: {}\n");
        return s;
    }

    s.push_str("paths:\n");
    for (path_str, methods) in &by_path {
        s.push_str(&format!("  {path_str}:\n"));
        let path_params = extract_path_params(path_str);
        for (http, m) in methods {
            s.push_str(&format!("    {http}:\n"));
            let summary = m
                .javadoc_text
                .as_deref()
                .map(first_line)
                .filter(|l| !l.is_empty())
                .unwrap_or_else(|| m.name.clone());
            s.push_str(&format!("      summary: {}\n", quote_yaml(&summary)));
            s.push_str(&format!("      operationId: {}\n", m.name));
            if !path_params.is_empty() {
                s.push_str("      parameters:\n");
                for p in &path_params {
                    s.push_str(&format!("        - name: {p}\n"));
                    s.push_str("          in: path\n");
                    s.push_str("          required: true\n");
                    s.push_str("          schema:\n");
                    s.push_str("            type: string\n");
                }
            }
            s.push_str("      responses:\n");
            s.push_str("        '200':\n");
            s.push_str("          description: TODO — refine via /spec-delta\n");
        }
    }
    s
}

fn render_domain_model(slug: &str) -> String {
    format!(
        "# {slug} domain model\n\
         \n\
         <!-- Auto-scaffolded by `jkit migrate scaffold-docs`. Refine via /spec-delta. -->\n\
         \n\
         ## Entities\n\
         \n\
         <!-- TODO: list entities, fields, relationships. spec-delta will refine this. -->\n\
         \n\
         ## Relationships\n\
         \n\
         <!-- TODO -->\n"
    )
}

fn render_impl_logic(slug: &str, controllers: &[&Controller]) -> String {
    let mut s = format!(
        "# {slug} implementation logic\n\
         \n\
         <!-- Auto-scaffolded by `jkit migrate scaffold-docs`. Refine via /spec-delta. -->\n\
         \n"
    );
    for c in controllers {
        s.push_str(&format!("## {}\n\n", c.class));
        if c.methods.is_empty() {
            s.push_str("- <!-- no @RequestMapping methods detected -->\n\n");
            continue;
        }
        for m in &c.methods {
            let endpoint = match (&m.http_method, &m.path) {
                (Some(h), Some(p)) => format!("`{h} {p}`"),
                _ => "non-routed".to_string(),
            };
            let desc = m
                .javadoc_text
                .as_deref()
                .map(first_line)
                .filter(|l| !l.is_empty())
                .unwrap_or_else(|| "TODO".to_string());
            s.push_str(&format!("- `{}` ({}) — {}\n", m.name, endpoint, desc));
        }
        s.push('\n');
    }
    s
}

fn render_scenarios(slug: &str, controllers: &[&Controller]) -> String {
    // Format matches `kit scenarios sync` (kit-core's canonical writer):
    // top-level YAML sequence of {endpoint, id, description}. Same shape
    // `jkit scenarios gap` reads.
    let mut s = format!(
        "# Scenarios for {slug}. Auto-scaffolded by `jkit migrate scaffold-docs`.\n\
         # Refine via /spec-delta + scenario-tdd.\n"
    );
    let mut emitted = false;
    for c in controllers {
        for m in &c.methods {
            if let (Some(http), Some(p)) = (&m.http_method, &m.path) {
                emitted = true;
                s.push_str(&format!("- endpoint: {http} {p}\n"));
                s.push_str(&format!("  id: {}-happy\n", m.name));
                s.push_str(&format!(
                    "  description: {}\n",
                    quote_yaml(&format!("{} happy path", m.name))
                ));
                s.push_str("  # TODO: add error / auth / validation cases\n");
            }
        }
    }
    if !emitted {
        // Empty list — keep YAML valid.
        s.push_str("[]\n");
    }
    s
}

fn load_domain_map(
    path: &Path,
    detected: &[Controller],
    warnings: &mut Vec<String>,
) -> Result<HashMap<String, String>> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("reading {}", path.display()))?;
    let parsed: DomainMap = serde_json::from_str(&raw)
        .with_context(|| format!("parsing {} as domain-map JSON", path.display()))?;

    let mut class_to_slug: HashMap<String, String> = HashMap::new();
    for d in &parsed.domains {
        for c in &d.controllers {
            if let Some(prev) = class_to_slug.insert(c.clone(), d.slug.clone()) {
                if prev != d.slug {
                    warnings.push(format!(
                        "domain map: controller {c} listed in both '{prev}' and '{}'",
                        d.slug
                    ));
                }
            }
        }
    }

    let detected_classes: std::collections::HashSet<String> =
        detected.iter().map(|c| c.class.clone()).collect();
    let mapped_classes: std::collections::HashSet<String> =
        class_to_slug.keys().cloned().collect();

    let missing: Vec<&String> = detected_classes.difference(&mapped_classes).collect();
    let stale: Vec<&String> = mapped_classes.difference(&detected_classes).collect();

    for m in &missing {
        warnings.push(format!(
            "domain map: controller {m} not listed; falling back to derive_domain_slug"
        ));
        if let Some(c) = detected.iter().find(|c| &c.class == *m) {
            class_to_slug.insert(c.class.clone(), c.domain_slug.clone());
        }
    }
    for s in &stale {
        warnings.push(format!(
            "domain map: controller {s} listed but not detected in source — ignored"
        ));
    }

    Ok(class_to_slug)
}

fn extract_path_params(path: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = path.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            let start = i + 1;
            while i < bytes.len() && bytes[i] != b'}' {
                i += 1;
            }
            if i <= bytes.len() && start < i {
                let name = &path[start..i];
                if !name.is_empty() && !out.iter().any(|n: &String| n == name) {
                    out.push(name.to_string());
                }
            }
        }
        i += 1;
    }
    out
}

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or("").trim().to_string()
}

fn quote_yaml(s: &str) -> String {
    // Always double-quote auto-generated strings — javadoc fragments can
    // contain colons, hashes, leading dashes, etc. that break bare YAML scalars.
    let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_path_params() {
        assert_eq!(extract_path_params("/users/{id}"), vec!["id"]);
        assert_eq!(
            extract_path_params("/users/{userId}/orders/{orderId}"),
            vec!["userId", "orderId"]
        );
        assert_eq!(extract_path_params("/users"), Vec::<String>::new());
        assert_eq!(extract_path_params("/users/{id}/{id}"), vec!["id"]);
    }

    #[test]
    fn quotes_yaml_safely() {
        assert_eq!(quote_yaml("hello"), "\"hello\"");
        assert_eq!(quote_yaml("foo: bar"), "\"foo: bar\"");
        assert_eq!(quote_yaml("she said \"hi\""), "\"she said \\\"hi\\\"\"");
    }

    #[test]
    fn first_line_strips_trailing() {
        assert_eq!(first_line("first\nsecond"), "first");
        assert_eq!(first_line("  spaced  "), "spaced");
        assert_eq!(first_line(""), "");
    }
}
