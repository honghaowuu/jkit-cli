use anyhow::{Context, Result};
use heck::{ToPascalCase, ToLowerCamelCase};
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

use crate::util::print_json;

#[derive(Debug, Deserialize)]
struct ScenariosFile(Vec<EndpointGroup>);

#[derive(Debug, Deserialize)]
struct EndpointGroup {
    endpoint: String,
    scenarios: Vec<Scenario>,
}

#[derive(Debug, Deserialize, Clone)]
struct Scenario {
    id: String,
    description: String,
}

#[derive(Debug, Serialize)]
struct GapEntrySingle<'a> {
    endpoint: &'a str,
    id: &'a str,
    description: &'a str,
}

#[derive(Debug, Serialize)]
struct GapEntryAggregated {
    domain: String,
    endpoint: String,
    id: String,
    description: String,
    test_class_path: Option<String>,
    test_method_name: String,
}

#[derive(Debug, Deserialize)]
struct SkippedScenarios {
    skipped: Vec<SkippedEntry>,
}

#[derive(Debug, Deserialize)]
struct SkippedEntry {
    domain: String,
    endpoint: String,
    id: String,
}

pub fn run(
    domain: Option<&str>,
    run_dir: Option<&Path>,
    test_root: &Path,
    pom_path: &Path,
) -> Result<()> {
    if let Some(run_dir) = run_dir {
        run_aggregated(run_dir, test_root, pom_path)
    } else if let Some(d) = domain {
        run_single(d, test_root)
    } else {
        anyhow::bail!("either <domain> or --run <dir> must be supplied");
    }
}

fn run_single(domain: &str, test_root: &Path) -> Result<()> {
    let yaml_path = PathBuf::from("docs/domains").join(domain).join("test-scenarios.yaml");
    if !yaml_path.exists() {
        // Per PRD: missing → output [], exit 0.
        println!("[]");
        return Ok(());
    }
    let groups = load_scenarios(&yaml_path)?;
    let implemented = implemented_method_names(test_root)?;

    let mut out: Vec<GapEntrySingle> = Vec::new();
    for g in &groups {
        for s in &g.scenarios {
            let camel = id_to_camel(&s.id);
            if !implemented.contains(&camel) {
                out.push(GapEntrySingle {
                    endpoint: &g.endpoint,
                    id: &s.id,
                    description: &s.description,
                });
            }
        }
    }
    print_json(&out)
}

fn run_aggregated(run_dir: &Path, test_root: &Path, pom_path: &Path) -> Result<()> {
    let cs_path = run_dir.join("change-summary.md");
    if !cs_path.exists() {
        anyhow::bail!("change-summary.md missing in {}", run_dir.display());
    }
    let cs_text = fs::read_to_string(&cs_path)
        .with_context(|| format!("reading {}", cs_path.display()))?;
    let domains = parse_affected_domains(&cs_text)
        .ok_or_else(|| anyhow::anyhow!("change-summary.md: '## Domains Changed' section not found"))?;

    let skipped: HashSet<(String, String, String)> = {
        let p = run_dir.join("skipped-scenarios.json");
        if p.exists() {
            let text = fs::read_to_string(&p)?;
            let s: SkippedScenarios = serde_json::from_str(&text)
                .with_context(|| format!("parsing {}", p.display()))?;
            s.skipped
                .into_iter()
                .map(|e| (e.domain, e.endpoint, e.id))
                .collect()
        } else {
            HashSet::new()
        }
    };

    let implemented = implemented_method_names(test_root)?;
    let pom_meta = read_pom_coords(pom_path).ok();

    let mut out: Vec<GapEntryAggregated> = Vec::new();
    for domain in &domains {
        let yaml_path = PathBuf::from("docs/domains").join(domain).join("test-scenarios.yaml");
        if !yaml_path.exists() {
            continue;
        }
        let groups = load_scenarios(&yaml_path)?;
        for g in &groups {
            for s in &g.scenarios {
                let camel = id_to_camel(&s.id);
                if implemented.contains(&camel) {
                    continue;
                }
                let key = (domain.clone(), g.endpoint.clone(), s.id.clone());
                if skipped.contains(&key) {
                    continue;
                }
                let test_class_path =
                    resolve_test_class_path(test_root, domain, pom_meta.as_ref());
                out.push(GapEntryAggregated {
                    domain: domain.clone(),
                    endpoint: g.endpoint.clone(),
                    id: s.id.clone(),
                    description: s.description.clone(),
                    test_class_path,
                    test_method_name: camel,
                });
            }
        }
    }
    print_json(&out)
}

fn load_scenarios(path: &Path) -> Result<Vec<EndpointGroup>> {
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let parsed: ScenariosFile = serde_yaml::from_str(&text)
        .with_context(|| format!("parsing {}", path.display()))?;
    Ok(parsed.0)
}

/// Convert a kebab-case id to camelCase.
pub fn id_to_camel(id: &str) -> String {
    id.to_lower_camel_case()
}

/// Walk the test root, collect every method name appearing in `void <name>(`.
fn implemented_method_names(test_root: &Path) -> Result<HashSet<String>> {
    let mut set = HashSet::new();
    if !test_root.exists() {
        return Ok(set);
    }
    let re = Regex::new(r"\bvoid\s+([A-Za-z_][A-Za-z0-9_]*)\s*\(").unwrap();
    for entry in WalkDir::new(test_root).into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if !path
            .file_name()
            .map(|n| n.to_string_lossy().ends_with("Test.java"))
            .unwrap_or(false)
        {
            continue;
        }
        let text = match fs::read_to_string(path) {
            Ok(t) => t,
            Err(_) => continue,
        };
        for cap in re.captures_iter(&text) {
            set.insert(cap[1].to_string());
        }
    }
    Ok(set)
}

/// Parse the first column of the markdown table under `## Domains Changed`.
pub fn parse_affected_domains(md: &str) -> Option<Vec<String>> {
    let parser = Parser::new_ext(md, Options::ENABLE_TABLES);
    let mut in_section = false;
    let mut current_heading_text = String::new();
    let mut in_heading = false;
    let mut in_table = false;
    let mut in_head = false;
    let mut col_idx: usize = 0;
    let mut current_cell = String::new();
    let mut domains: Vec<String> = Vec::new();

    for ev in parser {
        match ev {
            Event::Start(Tag::Heading { .. }) => {
                in_heading = true;
                current_heading_text.clear();
            }
            Event::End(TagEnd::Heading(_)) => {
                in_heading = false;
                let lower = current_heading_text.trim().to_lowercase();
                if lower == "domains changed" {
                    in_section = true;
                } else if in_section {
                    in_section = false;
                }
            }
            Event::Start(Tag::Table(_)) if in_section => {
                in_table = true;
            }
            Event::End(TagEnd::Table) if in_section => {
                in_table = false;
            }
            Event::Start(Tag::TableHead) if in_table => {
                in_head = true;
                col_idx = 0;
            }
            Event::End(TagEnd::TableHead) if in_table => {
                in_head = false;
            }
            Event::Start(Tag::TableRow) if in_table => {
                col_idx = 0;
            }
            Event::Start(Tag::TableCell) if in_table => {
                current_cell.clear();
            }
            Event::End(TagEnd::TableCell) if in_table => {
                if !in_head && col_idx == 0 {
                    let v = current_cell.trim().to_string();
                    if !v.is_empty() {
                        domains.push(v);
                    }
                }
                col_idx += 1;
            }
            Event::Text(t) | Event::Code(t) => {
                if in_heading {
                    current_heading_text.push_str(&t);
                } else if in_table {
                    current_cell.push_str(&t);
                }
            }
            _ => {}
        }
    }
    if domains.is_empty() {
        None
    } else {
        Some(domains)
    }
}

#[derive(Debug, Clone)]
struct PomCoords {
    group_id: String,
    artifact_id: String,
}

fn read_pom_coords(pom_path: &Path) -> Result<PomCoords> {
    let text = fs::read_to_string(pom_path)?;
    // Read top-level <groupId>/<artifactId> (skip <parent>'s).
    let stripped = strip_parent_block(&text);
    let group_id = first_inner(&stripped, "groupId").unwrap_or_default();
    let artifact_id = first_inner(&stripped, "artifactId").unwrap_or_default();
    if group_id.is_empty() || artifact_id.is_empty() {
        anyhow::bail!("pom.xml lacks <groupId> or <artifactId>");
    }
    Ok(PomCoords { group_id, artifact_id })
}

fn strip_parent_block(text: &str) -> String {
    if let (Some(s), Some(e_rel)) = (text.find("<parent>"), text.find("</parent>")) {
        if e_rel > s {
            let end = e_rel + "</parent>".len();
            let mut out = String::with_capacity(text.len());
            out.push_str(&text[..s]);
            out.push_str(&text[end..]);
            return out;
        }
    }
    text.to_string()
}

fn first_inner(text: &str, tag: &str) -> Option<String> {
    let open = format!("<{}>", tag);
    let close = format!("</{}>", tag);
    let i = text.find(&open)? + open.len();
    let j = text[i..].find(&close)? + i;
    Some(text[i..j].trim().to_string())
}

/// Either the first matching `*<DomainPascal>IntegrationTest.java` under test_root,
/// or the convention path derived from pom coords. None when pom unreadable.
fn resolve_test_class_path(
    test_root: &Path,
    domain: &str,
    pom: Option<&PomCoords>,
) -> Option<String> {
    let pascal = domain.to_pascal_case();
    let target_suffix = format!("{}IntegrationTest.java", pascal);
    if test_root.exists() {
        for entry in WalkDir::new(test_root).into_iter().filter_map(|e| e.ok()) {
            if !entry.file_type().is_file() {
                continue;
            }
            if let Some(name) = entry.path().file_name().and_then(|n| n.to_str()) {
                if name.ends_with(&target_suffix) {
                    return Some(entry.path().to_string_lossy().into_owned());
                }
            }
        }
    }
    let coords = pom?;
    let group_path = coords.group_id.replace('.', "/");
    let p = PathBuf::from(test_root)
        .join(&group_path)
        .join(&coords.artifact_id)
        .join(domain)
        .join(format!("{}IntegrationTest.java", pascal));
    Some(p.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camel_case_basic() {
        assert_eq!(id_to_camel("happy-path"), "happyPath");
        assert_eq!(id_to_camel("validation-empty-list"), "validationEmptyList");
        assert_eq!(id_to_camel("single"), "single");
    }

    #[test]
    fn parses_domains_table() {
        let md = "# Foo\n\n## Domains Changed\n\n| Domain | Notes |\n|---|---|\n| billing | new endpoint |\n| inventory | tweak |\n\n## Other\n";
        let v = parse_affected_domains(md).unwrap();
        assert_eq!(v, vec!["billing", "inventory"]);
    }
}
