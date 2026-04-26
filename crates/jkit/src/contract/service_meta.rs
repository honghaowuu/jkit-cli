use anyhow::{Context, Result};
use heck::ToKebabCase;
use regex::Regex;
use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

use crate::domain_layout::{self, ApiType};
use crate::util::print_json;

#[derive(Serialize)]
pub struct ServiceMeta {
    pub service_name: String,
    pub service_name_source: String,
    pub group_id: String,
    pub artifact_id: String,
    pub version: String,
    pub parent_artifact_id: Option<String>,
    pub sdk: Option<SdkInfo>,
    pub authentication_hint: Option<String>,
    pub controllers: Vec<Controller>,
    pub javadoc_quality: JavadocSummary,
    pub interview_drafts: InterviewDrafts,
    pub warnings: Vec<String>,
}

#[derive(Serialize)]
pub struct SdkInfo {
    pub present: bool,
    pub artifact_id: String,
    pub version: String,
}

#[derive(Serialize, Clone)]
pub struct Controller {
    pub class: String,
    pub file: String,
    pub domain_slug: String,
    /// Classified from the source file path via `domain_layout::classify_path`
    /// using the project's `docs/api-type-mapping.yaml` overrides (or Spring
    /// defaults when absent). Defaults to `WebApi` when no path component
    /// matches; a warning is emitted on `ServiceMeta.warnings` in that case.
    pub api_type: ApiType,
    pub methods: Vec<MethodMeta>,
}

#[derive(Serialize, Clone)]
pub struct MethodMeta {
    pub name: String,
    pub http_method: Option<String>,
    pub path: Option<String>,
    pub javadoc_quality: String,
    pub javadoc_text: Option<String>,
}

#[derive(Serialize)]
pub struct JavadocSummary {
    pub score: f64,
    pub total_methods: usize,
    pub missing: Vec<String>,
    pub thin: Vec<String>,
}

#[derive(Serialize, Default)]
pub struct InterviewDrafts {
    pub description: String,
    pub keywords: Vec<String>,
    pub invariants: Vec<String>,
    pub use_when: Vec<String>,
    pub not_responsible_for: Vec<String>,
}

pub fn run(pom_path: &Path, src_root: &Path) -> Result<()> {
    let meta = compute(pom_path, src_root)?;
    print_json(&meta)
}

pub fn compute(pom_path: &Path, src_root: &Path) -> Result<ServiceMeta> {
    let pom = fs::read_to_string(pom_path)
        .with_context(|| format!("reading {}", pom_path.display()))?;
    let coords = parse_pom_coords(&pom)?;

    let (service_name, service_name_source, mut warnings) =
        resolve_service_name(&coords);

    let mut controllers = if src_root.exists() {
        scan_controllers(src_root)?
    } else {
        Vec::new()
    };

    // Classify each controller's API type from its file path. The project's
    // mapping lives at `<project>/docs/api-type-mapping.yaml`; the project root
    // is taken as `pom_path.parent()` (or cwd when pom is at the workspace
    // root). Mapping load failures degrade silently to defaults — never block
    // service-meta on a malformed config file.
    let project_root = pom_path.parent().unwrap_or(Path::new("."));
    let api_type_mapping =
        domain_layout::load_mapping_from_project(project_root).unwrap_or_default();
    for c in &mut controllers {
        match domain_layout::classify_path(Path::new(&c.file), &api_type_mapping) {
            Some(ty) => c.api_type = ty,
            None => {
                warnings.push(format!(
                    "controller {} at {} has no recognizable API-type directory in its path; defaulting to web-api",
                    c.class, c.file
                ));
                // c.api_type already defaults to WebApi from scan_controllers
            }
        }
    }

    // Collisions on domain slug -> warn.
    let mut by_slug: HashMap<String, usize> = HashMap::new();
    for c in &controllers {
        *by_slug.entry(c.domain_slug.clone()).or_default() += 1;
    }
    for (slug, count) in &by_slug {
        if *count > 1 {
            warnings.push(format!(
                "domain slug collision: {} controllers map to '{}'",
                count, slug
            ));
        }
    }

    // Javadoc score
    let mut total = 0usize;
    let mut good = 0usize;
    let mut missing: Vec<String> = Vec::new();
    let mut thin: Vec<String> = Vec::new();
    for c in &controllers {
        for m in &c.methods {
            total += 1;
            let key = format!(
                "{}#{}",
                c.class.rsplit('.').next().unwrap_or(&c.class),
                m.name
            );
            match m.javadoc_quality.as_str() {
                "missing" => missing.push(key),
                "thin" => thin.push(key),
                _ => good += 1,
            }
        }
    }
    let score = if total == 0 {
        1.0
    } else {
        good as f64 / total as f64
    };

    let sdk = detect_sdk(pom_path, &coords);
    let authentication_hint = detect_auth_hint(src_root);

    let drafts = build_drafts(&service_name, &controllers);

    Ok(ServiceMeta {
        service_name,
        service_name_source,
        group_id: coords.group_id.clone(),
        artifact_id: coords.artifact_id.clone(),
        version: coords.version.clone(),
        parent_artifact_id: coords.parent_artifact_id.clone(),
        sdk,
        authentication_hint,
        controllers,
        javadoc_quality: JavadocSummary {
            score,
            total_methods: total,
            missing,
            thin,
        },
        interview_drafts: drafts,
        warnings,
    })
}

#[derive(Debug, Clone)]
pub struct PomCoords {
    pub group_id: String,
    pub artifact_id: String,
    pub version: String,
    pub parent_artifact_id: Option<String>,
}

pub fn parse_pom_coords(pom: &str) -> Result<PomCoords> {
    let parent_artifact_id = parent_inner(pom, "artifactId");

    let stripped = strip_parent_block(pom);
    let group_id = first_inner(&stripped, "groupId").unwrap_or_default();
    let artifact_id = first_inner(&stripped, "artifactId").unwrap_or_default();
    let version = first_inner(&stripped, "version").unwrap_or_default();
    if artifact_id.is_empty() {
        anyhow::bail!("pom.xml lacks <artifactId>");
    }
    Ok(PomCoords {
        group_id,
        artifact_id,
        version,
        parent_artifact_id,
    })
}

fn parent_inner(text: &str, tag: &str) -> Option<String> {
    let s = text.find("<parent>")?;
    let e = text[s..].find("</parent>")? + s;
    let block = &text[s..e];
    let open = format!("<{}>", tag);
    let close = format!("</{}>", tag);
    let i = block.find(&open)? + open.len();
    let j = block[i..].find(&close)? + i;
    Some(block[i..j].trim().to_string())
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

fn resolve_service_name(coords: &PomCoords) -> (String, String, Vec<String>) {
    let mut warnings: Vec<String> = Vec::new();
    let resources = Path::new("src/main/resources");
    if resources.is_dir() {
        let mut yaml_files: Vec<PathBuf> = Vec::new();
        let mut prop_files: Vec<PathBuf> = Vec::new();
        for entry in fs::read_dir(resources).into_iter().flatten().flatten() {
            let p = entry.path();
            if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                if name.starts_with("application")
                    && (name.ends_with(".yaml") || name.ends_with(".yml"))
                {
                    yaml_files.push(p.clone());
                }
                if name.starts_with("application") && name.ends_with(".properties") {
                    prop_files.push(p.clone());
                }
            }
        }
        // sort: base file first, then profile-specific files (most-specific last)
        yaml_files.sort();
        prop_files.sort();

        let mut value: Option<(String, String)> = None;
        for p in yaml_files.iter().chain(prop_files.iter()) {
            let text = match fs::read_to_string(p) {
                Ok(t) => t,
                Err(_) => continue,
            };
            if let Some(v) = extract_spring_app_name(&text) {
                let source = p
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("application")
                    .to_string();
                value = Some((v, source));
            }
        }
        if let Some((v, source)) = value {
            if v.contains("${") {
                warnings.push(format!(
                    "spring.application.name in {} is interpolated ({}); falling back to artifactId",
                    source, v
                ));
                return (
                    coords.artifact_id.clone(),
                    "interpolated".to_string(),
                    warnings,
                );
            }
            return (v, format!("spring_application_name:{}", source), warnings);
        }
    }
    (coords.artifact_id.clone(), "artifact_id".to_string(), warnings)
}

/// Look for `spring.application.name: <value>` in either yaml or properties form.
fn extract_spring_app_name(text: &str) -> Option<String> {
    // Properties: spring.application.name=<val>
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("spring.application.name") {
            let rest = rest.trim_start();
            if let Some(rest) = rest.strip_prefix('=') {
                return Some(rest.trim().trim_matches(|c| c == '"' || c == '\'').to_string());
            }
            if let Some(rest) = rest.strip_prefix(':') {
                return Some(rest.trim().trim_matches(|c| c == '"' || c == '\'').to_string());
            }
        }
    }
    // YAML walk: look for `spring:` then `application:` then `name:`.
    let mut current_indent_stack: Vec<(usize, String)> = Vec::new();
    for raw in text.lines() {
        let trimmed = raw.trim_end();
        if trimmed.trim().is_empty() || trimmed.trim_start().starts_with('#') {
            continue;
        }
        let indent = trimmed.chars().take_while(|c| *c == ' ').count();
        let stripped = trimmed.trim_start();
        // Pop deeper-or-equal levels.
        while let Some(&(d, _)) = current_indent_stack.last() {
            if d >= indent {
                current_indent_stack.pop();
            } else {
                break;
            }
        }
        if let Some((key, value)) = stripped.split_once(':') {
            let key = key.trim();
            let value = value.trim();
            current_indent_stack.push((indent, key.to_string()));
            let path: Vec<&str> = current_indent_stack
                .iter()
                .map(|(_, k)| k.as_str())
                .collect();
            if path.as_slice() == ["spring", "application", "name"] {
                let v = value.trim_matches(|c| c == '"' || c == '\'').to_string();
                if !v.is_empty() {
                    return Some(v);
                }
            }
        }
    }
    None
}

fn scan_controllers(src_root: &Path) -> Result<Vec<Controller>> {
    let class_re = Regex::new(
        r#"(?ms)((?:/\*\*.*?\*/\s*)?)(?:@\w+(?:\([^)]*\))?\s*)*?@(?P<ann>RestController|Controller)(?:\([^)]*\))?\s*(?:@\w+(?:\([^)]*\))?\s*)*?(?:public\s+|abstract\s+|final\s+)*class\s+(?P<name>\w+)"#,
    )
    .unwrap();
    let mapping_re = Regex::new(
        r#"@RequestMapping\(\s*(?:value\s*=\s*)?"(?P<p>[^"]*)""#,
    )
    .unwrap();
    let method_re = Regex::new(
        r#"(?ms)(?P<jd>(?:/\*\*.*?\*/\s*)?)(?:@(?P<http>GetMapping|PostMapping|PutMapping|DeleteMapping|PatchMapping|RequestMapping)(?:\(\s*(?:value\s*=\s*)?"(?P<mp>[^"]*)"[^)]*\))?\s*)?public\s+(?:[\w<>?\[\],\s]+?)\s+(?P<name>\w+)\s*\([^)]*\)"#,
    )
    .unwrap();

    let mut out = Vec::new();
    for entry in WalkDir::new(src_root).into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() {
            continue;
        }
        if entry.path().extension().and_then(|s| s.to_str()) != Some("java") {
            continue;
        }
        let text = match fs::read_to_string(entry.path()) {
            Ok(t) => t,
            Err(_) => continue,
        };

        // package + class names
        let pkg_re = Regex::new(r"package\s+([\w.]+)\s*;").unwrap();
        let pkg = pkg_re
            .captures(&text)
            .map(|c| c[1].to_string())
            .unwrap_or_default();

        for cap in class_re.captures_iter(&text) {
            let name = &cap["name"];
            let fqcn = if pkg.is_empty() {
                name.to_string()
            } else {
                format!("{}.{}", pkg, name)
            };
            // Get class body to scan methods within.
            let class_pos = cap.get(0).unwrap().end();
            let body = match grab_class_body(&text, class_pos) {
                Some(b) => b,
                None => continue,
            };
            // Class-level @RequestMapping path (best-effort).
            let class_path = mapping_re
                .captures(&text[..class_pos])
                .map(|c| c["p"].to_string())
                .unwrap_or_default();

            let mut methods = Vec::new();
            for m in method_re.captures_iter(body) {
                let mname = m["name"].to_string();
                if mname == name {
                    continue;
                }
                let http = m
                    .name("http")
                    .map(|s| match s.as_str() {
                        "GetMapping" => "GET",
                        "PostMapping" => "POST",
                        "PutMapping" => "PUT",
                        "DeleteMapping" => "DELETE",
                        "PatchMapping" => "PATCH",
                        _ => "ANY",
                    })
                    .map(|s| s.to_string());
                let mp = m.name("mp").map(|s| s.as_str().to_string()).unwrap_or_default();
                let path = if class_path.is_empty() && mp.is_empty() {
                    None
                } else {
                    Some(format!("{}{}", class_path, mp))
                };
                let jd = m.name("jd").map(|s| s.as_str()).unwrap_or("");
                let jd_text = parse_javadoc(jd);
                let quality = score_javadoc(&mname, jd_text.as_deref());
                methods.push(MethodMeta {
                    name: mname,
                    http_method: http,
                    path,
                    javadoc_quality: quality,
                    javadoc_text: jd_text,
                });
            }
            let domain_slug = derive_domain_slug(name);
            if !name.ends_with("Controller") {
                // surfaced via warnings later; record by emitting it anyway.
            }
            out.push(Controller {
                class: fqcn,
                file: entry.path().to_string_lossy().into_owned(),
                domain_slug,
                // Initial placeholder — `compute()` reclassifies with the
                // project's loaded mapping after scan_controllers returns.
                api_type: ApiType::WebApi,
                methods,
            });
        }
    }
    Ok(out)
}

fn grab_class_body(text: &str, start: usize) -> Option<&str> {
    let bytes = text.as_bytes();
    let mut i = start;
    while i < bytes.len() && bytes[i] != b'{' {
        i += 1;
    }
    if i >= bytes.len() {
        return None;
    }
    let body_start = i + 1;
    let mut depth = 1i32;
    i += 1;
    while i < bytes.len() && depth > 0 {
        match bytes[i] {
            b'{' => depth += 1,
            b'}' => depth -= 1,
            _ => {}
        }
        i += 1;
    }
    Some(&text[body_start..i.saturating_sub(1)])
}

fn parse_javadoc(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if !raw.starts_with("/**") {
        return None;
    }
    let inner = raw
        .trim_start_matches("/**")
        .trim_end_matches("*/");
    let mut lines: Vec<String> = Vec::new();
    for line in inner.lines() {
        let trimmed = line.trim().trim_start_matches('*').trim();
        if trimmed.starts_with('@') {
            break;
        }
        if !trimmed.is_empty() {
            lines.push(trimmed.to_string());
        }
    }
    let joined = lines.join(" ");
    if joined.is_empty() {
        None
    } else {
        Some(joined)
    }
}

pub fn score_javadoc(method_name: &str, doc: Option<&str>) -> String {
    let doc = match doc {
        Some(d) if !d.trim().is_empty() => d.trim().to_string(),
        _ => return "missing".to_string(),
    };
    let words = doc.split_whitespace().count();
    if words < 5 {
        return "thin".to_string();
    }
    let lower_doc = doc.to_lowercase();
    let lower_name = method_name.to_lowercase();
    if lower_doc.contains(&lower_name) && words < 8 {
        return "thin".to_string();
    }
    "good".to_string()
}

pub fn derive_domain_slug(class_name: &str) -> String {
    let stripped = class_name
        .strip_suffix("Controller")
        .unwrap_or(class_name);
    if stripped.is_empty() {
        return class_name.to_kebab_case();
    }
    stripped.to_kebab_case()
}

fn detect_sdk(pom_path: &Path, _coords: &PomCoords) -> Option<SdkInfo> {
    // Look in sibling modules under the parent dir for an artifactId ending in -api or -sdk.
    let parent_dir = pom_path.parent()?.parent()?;
    if !parent_dir.is_dir() {
        return None;
    }
    for entry in fs::read_dir(parent_dir).into_iter().flatten().flatten() {
        let p = entry.path();
        let sibling_pom = p.join("pom.xml");
        if !sibling_pom.exists() {
            continue;
        }
        if let Ok(text) = fs::read_to_string(&sibling_pom) {
            let stripped = strip_parent_block(&text);
            if let Some(aid) = first_inner(&stripped, "artifactId") {
                if aid.ends_with("-api") || aid.ends_with("-sdk") {
                    let v = first_inner(&stripped, "version").unwrap_or_default();
                    return Some(SdkInfo {
                        present: true,
                        artifact_id: aid,
                        version: v,
                    });
                }
            }
        }
    }
    None
}

fn detect_auth_hint(src_root: &Path) -> Option<String> {
    if !src_root.exists() {
        return None;
    }
    let mut found_pre_auth = false;
    let mut found_security = false;
    for entry in WalkDir::new(src_root).into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() {
            continue;
        }
        if entry.path().extension().and_then(|s| s.to_str()) != Some("java") {
            continue;
        }
        if let Ok(text) = fs::read_to_string(entry.path()) {
            if text.contains("@PreAuthorize") || text.contains("@RolesAllowed") {
                found_pre_auth = true;
            }
            if text.contains("WebSecurityConfigurerAdapter")
                || text.contains("SecurityFilterChain")
                || text.contains("@EnableWebSecurity")
            {
                found_security = true;
            }
        }
    }
    if found_pre_auth || found_security {
        Some("Bearer".into())
    } else {
        None
    }
}

fn build_drafts(service_name: &str, controllers: &[Controller]) -> InterviewDrafts {
    let mut keywords: Vec<String> = vec![service_name.to_string()];
    for c in controllers {
        if !keywords.contains(&c.domain_slug) {
            keywords.push(c.domain_slug.clone());
        }
    }
    InterviewDrafts {
        description: format!("Service for {}.", service_name),
        keywords,
        invariants: vec![],
        use_when: vec![],
        not_responsible_for: vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn javadoc_missing_when_none() {
        assert_eq!(score_javadoc("createInvoice", None), "missing");
        assert_eq!(score_javadoc("createInvoice", Some("   ")), "missing");
    }

    #[test]
    fn javadoc_thin_when_short() {
        assert_eq!(score_javadoc("createInvoice", Some("creates an invoice")), "thin");
    }

    #[test]
    fn javadoc_good_when_descriptive() {
        let doc = "Creates a new invoice for the given tenant id.";
        assert_eq!(score_javadoc("createInvoice", Some(doc)), "good");
    }

    #[test]
    fn domain_slug_strips_controller_and_kebabs() {
        assert_eq!(derive_domain_slug("InvoiceController"), "invoice");
        assert_eq!(derive_domain_slug("BulkInvoiceController"), "bulk-invoice");
        assert_eq!(derive_domain_slug("PaymentRefundController"), "payment-refund");
    }

    #[test]
    fn extracts_yaml_app_name() {
        let yaml = "spring:\n  application:\n    name: billing\n";
        assert_eq!(extract_spring_app_name(yaml).as_deref(), Some("billing"));
    }

    #[test]
    fn extracts_properties_app_name() {
        let p = "spring.application.name=billing\n";
        assert_eq!(extract_spring_app_name(p).as_deref(), Some("billing"));
    }

    #[test]
    fn detects_interpolated() {
        let yaml = "spring:\n  application:\n    name: ${SERVICE_NAME}\n";
        assert!(extract_spring_app_name(yaml).unwrap().contains("${"));
    }
}
