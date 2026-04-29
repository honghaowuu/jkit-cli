use anyhow::{Context, Result};
use heck::{ToLowerCamelCase, ToPascalCase};
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use regex::Regex;
use serde::Serialize;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

use crate::scenarios_yaml::ScenariosFile;
use crate::util::print_json;

#[derive(Debug, Serialize)]
struct GapEntrySingle {
    endpoint: String,
    id: String,
    description: String,
    /// `Some(api_type)` when the gap came from `domains.<slug>.<api-type>`;
    /// `None` for flat (single-type) layouts.
    #[serde(skip_serializing_if = "Option::is_none")]
    api_type: Option<String>,
}

#[derive(Debug, Serialize)]
struct GapEntryAggregated {
    domain: String,
    endpoint: String,
    id: String,
    description: String,
    test_class_path: Option<String>,
    test_method_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    api_type: Option<String>,
}

/// `kit scenarios skip` writes a flat JSON array of records;
/// older designs used an envelope `{ "skipped": [...] }`. Accept both.
#[derive(Debug, serde::Deserialize)]
#[serde(untagged)]
enum SkippedScenarios {
    Envelope { skipped: Vec<SkippedEntry> },
    Flat(Vec<SkippedEntry>),
}

impl SkippedScenarios {
    fn into_entries(self) -> Vec<SkippedEntry> {
        match self {
            SkippedScenarios::Envelope { skipped } => skipped,
            SkippedScenarios::Flat(v) => v,
        }
    }
}

#[derive(Debug, serde::Deserialize)]
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
    let cwd = std::env::current_dir().context("reading cwd")?;
    if let Some(run_dir) = run_dir {
        run_aggregated(&cwd, run_dir, test_root, pom_path)
    } else if let Some(d) = domain {
        run_single(&cwd, d, test_root)
    } else {
        anyhow::bail!("either <domain> or --run <dir> must be supplied");
    }
}

fn run_single(project_root: &Path, domain: &str, test_root: &Path) -> Result<()> {
    let file = ScenariosFile::load(project_root)?;
    let section = file.section(domain)?;
    let implemented = implemented_method_names(test_root)?;
    let skipped = load_skip_records(&archived_run_dirs(project_root))?;

    let mut out: Vec<GapEntrySingle> = Vec::new();
    if let Some(section) = section {
        for (api_type, e) in section.iter_with_type() {
            let camel = id_to_camel(&e.id);
            if implemented.contains(&camel) {
                continue;
            }
            let key = (domain.to_string(), e.endpoint.clone(), e.id.clone());
            if skipped.contains(&key) {
                continue;
            }
            out.push(GapEntrySingle {
                endpoint: e.endpoint.clone(),
                id: e.id.clone(),
                description: e.description.clone(),
                api_type: api_type.map(str::to_string),
            });
        }
    }
    print_json(&serde_json::json!({ "gaps": out }))
}

fn run_aggregated(
    project_root: &Path,
    run_dir: &Path,
    test_root: &Path,
    pom_path: &Path,
) -> Result<()> {
    let cs_path = run_dir.join("change-summary.md");
    if !cs_path.exists() {
        anyhow::bail!("change-summary.md missing in {}", run_dir.display());
    }
    let cs_text =
        fs::read_to_string(&cs_path).with_context(|| format!("reading {}", cs_path.display()))?;
    let domains = parse_affected_domains(&cs_text)
        .ok_or_else(|| anyhow::anyhow!("change-summary.md: '## Domains Changed' section not found"))?;

    let skipped: HashSet<(String, String, String)> =
        load_skip_records(&[run_dir.to_path_buf()])?;

    let implemented = implemented_method_names(test_root)?;
    let pom_meta = read_pom_coords(pom_path).ok();

    let file = ScenariosFile::load(project_root)?;
    let mut out: Vec<GapEntryAggregated> = Vec::new();
    for domain in &domains {
        let section = match file.section(domain)? {
            Some(s) => s,
            None => continue,
        };
        for (api_type, e) in section.iter_with_type() {
            let camel = id_to_camel(&e.id);
            if implemented.contains(&camel) {
                continue;
            }
            let key = (domain.clone(), e.endpoint.clone(), e.id.clone());
            if skipped.contains(&key) {
                continue;
            }
            let test_class_path = resolve_test_class_path(test_root, domain, pom_meta.as_ref());
            out.push(GapEntryAggregated {
                domain: domain.clone(),
                endpoint: e.endpoint.clone(),
                id: e.id.clone(),
                description: e.description.clone(),
                test_class_path,
                test_method_name: camel,
                api_type: api_type.map(str::to_string),
            });
        }
    }
    print_json(&serde_json::json!({ "gaps": out }))
}

/// Load + union skip records from each given run directory's
/// `skipped-scenarios.json`. Tolerates both envelope and flat-array shapes.
fn load_skip_records(run_dirs: &[PathBuf]) -> Result<HashSet<(String, String, String)>> {
    let mut set: HashSet<(String, String, String)> = HashSet::new();
    for run in run_dirs {
        let p = run.join("skipped-scenarios.json");
        if !p.exists() {
            continue;
        }
        let text = fs::read_to_string(&p)?;
        let s: SkippedScenarios =
            serde_json::from_str(&text).with_context(|| format!("parsing {}", p.display()))?;
        for e in s.into_entries() {
            set.insert((e.domain, e.endpoint, e.id));
        }
    }
    Ok(set)
}

fn archived_run_dirs(project_root: &Path) -> Vec<PathBuf> {
    let done = project_root.join(".jkit/done");
    if !done.is_dir() {
        return Vec::new();
    }
    fs::read_dir(&done)
        .map(|it| {
            it.filter_map(|e| e.ok())
                .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
                .map(|e| e.path())
                .collect()
        })
        .unwrap_or_default()
}

/// Sanitize an identifier for use as a Java package segment.
fn sanitize_pkg_segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) {
        let mut prefixed = String::with_capacity(out.len() + 1);
        prefixed.push('_');
        prefixed.push_str(&out);
        out = prefixed;
    }
    out
}

/// Convert a kebab-case id to camelCase.
pub fn id_to_camel(id: &str) -> String {
    id.to_lower_camel_case()
}

/// Count unimplemented scenarios per domain, reading top-level
/// `docs/test-scenarios.yaml`. Domains absent from the file contribute 0.
pub fn count_gaps_per_domain(
    project_root: &Path,
    domains: &[String],
    test_root: &Path,
) -> Result<Vec<(String, usize)>> {
    let file = ScenariosFile::load(project_root)?;
    let implemented = implemented_method_names(test_root)?;
    let mut counts = Vec::with_capacity(domains.len());
    for domain in domains {
        let mut count = 0usize;
        if let Some(section) = file.section(domain)? {
            for (_ty, e) in section.iter_with_type() {
                if !implemented.contains(&id_to_camel(&e.id)) {
                    count += 1;
                }
            }
        }
        counts.push((domain.clone(), count));
    }
    Ok(counts)
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
    let stripped = strip_parent_block(&text);
    let group_id = first_inner(&stripped, "groupId").unwrap_or_default();
    let artifact_id = first_inner(&stripped, "artifactId").unwrap_or_default();
    if group_id.is_empty() || artifact_id.is_empty() {
        anyhow::bail!("pom.xml lacks <groupId> or <artifactId>");
    }
    Ok(PomCoords {
        group_id,
        artifact_id,
    })
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
    let group_path: String = coords
        .group_id
        .split('.')
        .map(sanitize_pkg_segment)
        .collect::<Vec<_>>()
        .join("/");
    let artifact_seg = sanitize_pkg_segment(&coords.artifact_id);
    let domain_seg = sanitize_pkg_segment(domain);
    let p = PathBuf::from(test_root)
        .join(&group_path)
        .join(&artifact_seg)
        .join(&domain_seg)
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

    #[test]
    fn count_gaps_reads_top_level_yaml() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("docs")).unwrap();
        std::fs::write(
            tmp.path().join("docs/test-scenarios.yaml"),
            r#"domains:
  billing:
    - endpoint: GET /a
      id: alpha-bravo
      description: ok
    - endpoint: GET /b
      id: charlie-delta
      description: ok
"#,
        )
        .unwrap();
        let counts = count_gaps_per_domain(
            tmp.path(),
            &["billing".to_string(), "missing".to_string()],
            Path::new("/nonexistent"),
        )
        .unwrap();
        assert_eq!(counts[0], ("billing".to_string(), 2));
        assert_eq!(counts[1], ("missing".to_string(), 0));
    }
}
