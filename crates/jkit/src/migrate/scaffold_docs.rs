use anyhow::{Context, Result};
use serde::Serialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use crate::contract::service_meta::{self, Controller, MethodMeta};
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
    files_created: Vec<String>,
    files_existing: Vec<String>,
    controllers: Vec<String>,
    method_count: usize,
}

const FILES: &[&str] = &[
    "api-spec.yaml",
    "domain-model.md",
    "api-implement-logic.md",
    "test-scenarios.yaml",
];

pub fn run(src_root: &Path, pom: &Path, out_dir: &Path) -> Result<()> {
    let meta = service_meta::compute(pom, src_root)
        .context("scanning controllers via service_meta::compute")?;

    let mut by_slug: BTreeMap<String, Vec<&Controller>> = BTreeMap::new();
    for c in &meta.controllers {
        by_slug.entry(c.domain_slug.clone()).or_default().push(c);
    }

    let mut report = ScaffoldReport {
        domains: Vec::new(),
        warnings: meta.warnings.clone(),
    };

    for (slug, controllers) in &by_slug {
        let dir = out_dir.join(slug);
        fs::create_dir_all(&dir)
            .with_context(|| format!("creating {}", dir.display()))?;

        let mut created = Vec::new();
        let mut existing = Vec::new();

        for fname in FILES {
            let path = dir.join(fname);
            if path.exists() {
                existing.push((*fname).to_string());
                continue;
            }
            let body = match *fname {
                "api-spec.yaml" => render_api_spec(slug, controllers),
                "domain-model.md" => render_domain_model(slug),
                "api-implement-logic.md" => render_impl_logic(slug, controllers),
                "test-scenarios.yaml" => render_scenarios(slug, controllers),
                _ => unreachable!(),
            };
            atomic_write(&path, body.as_bytes())
                .with_context(|| format!("writing {}", path.display()))?;
            created.push((*fname).to_string());
        }

        let method_count: usize = controllers.iter().map(|c| c.methods.len()).sum();
        report.domains.push(DomainReport {
            name: slug.clone(),
            dir: dir.display().to_string(),
            files_created: created,
            files_existing: existing,
            controllers: controllers.iter().map(|c| c.class.clone()).collect(),
            method_count,
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
    let mut s = format!(
        "# Scenarios for {slug}. Auto-scaffolded by `jkit migrate scaffold-docs`.\n\
         # Refine via /spec-delta + scenario-tdd.\n\
         scenarios:\n"
    );
    let mut emitted = false;
    for c in controllers {
        for m in &c.methods {
            if let (Some(http), Some(p)) = (&m.http_method, &m.path) {
                emitted = true;
                s.push_str(&format!("  - id: {}-happy\n", m.name));
                s.push_str(&format!("    endpoint: {http} {p}\n"));
                s.push_str(&format!(
                    "    description: {}\n",
                    quote_yaml(&format!("{} happy path", m.name))
                ));
                s.push_str("    # TODO: add error / auth / validation cases\n");
            }
        }
    }
    if !emitted {
        // Empty list — keep YAML valid.
        s.push_str("  []\n");
    }
    s
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
