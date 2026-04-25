use anyhow::Result;
use serde::Serialize;
use std::path::Path;

use super::xml::{
    child_text, dependencies_span, detect_indent, ensure_dependencies, indent_fragment,
    insert_into_span, read_pom, write_pom,
};
use crate::util::print_json;

#[derive(Serialize)]
struct AddDepFragment {
    id: String,
    present: bool,
    action: &'static str,
}

#[derive(Serialize)]
struct AddDepReport {
    profile: String,
    fragments: Vec<AddDepFragment>,
    missing: Vec<String>,
    already_present: Vec<String>,
    actions_taken: Vec<String>,
    ready: bool,
    blocking_errors: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    warnings: Vec<String>,
}

pub fn run(
    group_id: &str,
    artifact_id: &str,
    version: &str,
    scope: Option<&str>,
    apply: bool,
    pom_path: &Path,
) -> Result<()> {
    if group_id.is_empty() || artifact_id.is_empty() || version.is_empty() {
        anyhow::bail!("--group-id, --artifact-id, and --version must all be non-empty");
    }
    if !pom_path.exists() {
        anyhow::bail!("pom.xml not found at {}", pom_path.display());
    }
    let original = read_pom(pom_path)?;
    let indent = detect_indent(&original);
    let id = format!("{}:{}", group_id, artifact_id);

    let mut working = original.clone();
    let mut warnings: Vec<String> = Vec::new();
    let mut actions_taken: Vec<String> = Vec::new();
    let mut blocking_errors: Vec<String> = Vec::new();

    if dependencies_span(&working).is_none() {
        if apply {
            working = ensure_dependencies(&working, &indent)?;
        } else {
            // Dry-run: report dependencies_section_missing and continue.
            blocking_errors.push("dependencies_section_missing".to_string());
            let report = AddDepReport {
                profile: "add-dep".to_string(),
                fragments: vec![AddDepFragment {
                    id: id.clone(),
                    present: false,
                    action: "reported",
                }],
                missing: vec![id],
                already_present: vec![],
                actions_taken,
                ready: false,
                blocking_errors,
                warnings,
            };
            return print_json(&report);
        }
    }
    let span = dependencies_span(&working).expect("dependencies span ensured above");

    // Search for an existing dep with this group+artifact.
    // We need both presence and version-mismatch info.
    let inner = &working[span.inner_start..span.inner_end];
    let mut version_mismatch: Option<String> = None;
    let mut same_version = false;
    let mut present = false;
    {
        // walk dependency children manually (simple regex-ish via xml.rs::child_exists doesn't return body).
        use quick_xml::events::Event;
        use quick_xml::Reader;
        let mut reader = Reader::from_str(inner);
        reader.config_mut().trim_text(false);
        let mut buf = Vec::new();
        let mut depth: usize = 0;
        let mut start_byte: Option<usize> = None;
        loop {
            let pos_before = reader.buffer_position() as usize;
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(e)) => {
                    let name_owned = std::str::from_utf8(e.name().as_ref())
                        .unwrap_or("")
                        .to_string();
                    if depth == 0 && name_owned == "dependency" {
                        start_byte = Some(pos_before);
                    }
                    depth += 1;
                }
                Ok(Event::End(_)) => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        if let Some(s) = start_byte.take() {
                            let e_pos = reader.buffer_position() as usize;
                            let body = &inner[s..e_pos];
                            let g = child_text(body, "groupId");
                            let a = child_text(body, "artifactId");
                            if g.as_deref() == Some(group_id) && a.as_deref() == Some(artifact_id)
                            {
                                present = true;
                                let v = child_text(body, "version").unwrap_or_default();
                                if v == version {
                                    same_version = true;
                                } else {
                                    version_mismatch = Some(v);
                                }
                            }
                        }
                    }
                }
                Ok(Event::Eof) => break,
                Err(_) => break,
                _ => {}
            }
            buf.clear();
        }
    }
    let mut fragments = Vec::new();
    let mut missing = Vec::new();
    let mut already_present = Vec::new();

    if present && same_version {
        fragments.push(AddDepFragment {
            id: id.clone(),
            present: true,
            action: "skipped",
        });
        already_present.push(id.clone());
    } else if present && version_mismatch.is_some() {
        let existing = version_mismatch.clone().unwrap();
        fragments.push(AddDepFragment {
            id: id.clone(),
            present: true,
            action: "version_mismatch",
        });
        already_present.push(id.clone());
        warnings.push(format!(
            "{} present at version {} (requested {}); not modified",
            id, existing, version
        ));
    } else {
        // Insert.
        if apply {
            let child_indent = format!("{}{}", span.closing_indent, indent);
            let mut frag = String::new();
            frag.push_str("<dependency>\n");
            frag.push_str(&format!("    <groupId>{}</groupId>\n", group_id));
            frag.push_str(&format!("    <artifactId>{}</artifactId>\n", artifact_id));
            frag.push_str(&format!("    <version>{}</version>\n", version));
            if let Some(s) = scope {
                frag.push_str(&format!("    <scope>{}</scope>\n", s));
            }
            frag.push_str("</dependency>\n");
            let body = indent_fragment(&frag, &child_indent, &indent);
            working = insert_into_span(&working, &span, &body);
            fragments.push(AddDepFragment {
                id: id.clone(),
                present: false,
                action: "added",
            });
            actions_taken.push(format!("added {} to <dependencies>", id));
        } else {
            fragments.push(AddDepFragment {
                id: id.clone(),
                present: false,
                action: "reported",
            });
            missing.push(id.clone());
        }
    }

    if apply && working != original {
        write_pom(pom_path, &working)?;
    }

    let ready = missing.is_empty() && blocking_errors.is_empty();
    let report = AddDepReport {
        profile: "add-dep".to_string(),
        fragments,
        missing,
        already_present,
        actions_taken,
        ready,
        blocking_errors,
        warnings,
    };
    print_json(&report)
}
