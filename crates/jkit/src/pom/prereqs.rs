use anyhow::{Context, Result};
use serde::Serialize;
use std::path::Path;

use super::xml::{
    child_exists, child_text, dependencies_span, detect_indent, ensure_dependencies,
    ensure_plugins, indent_fragment, insert_into_span, plugins_span, read_pom, write_pom,
    ElementSpan,
};
use super::ProfileArg;
use crate::util::print_json;

/// One pom fragment to install: a dependency or plugin.
struct Fragment {
    /// Stable id used in JSON output.
    id: &'static str,
    kind: FragmentKind,
    /// Coordinates used to detect "already present".
    group_id: &'static str,
    artifact_id: &'static str,
    /// XML body, stored at zero indent (4-space internal indents).
    body: &'static str,
}

#[derive(Copy, Clone, Eq, PartialEq)]
enum FragmentKind {
    Dependency,
    Plugin,
}

const TESTCONTAINERS: &str = include_str!("../../templates/pom-fragments/testcontainers.xml");
const COMPOSE: &str = include_str!("../../templates/pom-fragments/compose.xml");
const JACOCO: &str = include_str!("../../templates/pom-fragments/jacoco.xml");
const QUALITY: &str = include_str!("../../templates/pom-fragments/quality.xml");
const SMART_DOC: &str = include_str!("../../templates/pom-fragments/smart-doc.xml");
const FLYWAY: &str = include_str!("../../templates/pom-fragments/flyway.xml");

fn profile_fragments(profile: ProfileArg) -> Vec<Fragment> {
    match profile {
        ProfileArg::Testcontainers => vec![
            Fragment {
                id: "testcontainers-junit-jupiter",
                kind: FragmentKind::Dependency,
                group_id: "org.testcontainers",
                artifact_id: "junit-jupiter",
                body: extract_block(TESTCONTAINERS, 0),
            },
            Fragment {
                id: "testcontainers-postgresql",
                kind: FragmentKind::Dependency,
                group_id: "org.testcontainers",
                artifact_id: "postgresql",
                body: extract_block(TESTCONTAINERS, 1),
            },
            Fragment {
                id: "rest-assured",
                kind: FragmentKind::Dependency,
                group_id: "io.rest-assured",
                artifact_id: "rest-assured",
                body: extract_block(TESTCONTAINERS, 2),
            },
            Fragment {
                id: "wiremock-standalone",
                kind: FragmentKind::Dependency,
                group_id: "org.wiremock",
                artifact_id: "wiremock-standalone",
                body: extract_block(TESTCONTAINERS, 3),
            },
            Fragment {
                id: "spring-boot-testcontainers",
                kind: FragmentKind::Dependency,
                group_id: "org.springframework.boot",
                artifact_id: "spring-boot-testcontainers",
                body: extract_block(TESTCONTAINERS, 4),
            },
        ],
        ProfileArg::Compose => vec![Fragment {
            id: "rest-assured",
            kind: FragmentKind::Dependency,
            group_id: "io.rest-assured",
            artifact_id: "rest-assured",
            body: COMPOSE,
        }],
        ProfileArg::Jacoco => vec![Fragment {
            id: "jacoco-maven-plugin",
            kind: FragmentKind::Plugin,
            group_id: "org.jacoco",
            artifact_id: "jacoco-maven-plugin",
            body: JACOCO,
        }],
        ProfileArg::Quality => vec![
            Fragment {
                id: "spotless-maven-plugin",
                kind: FragmentKind::Plugin,
                group_id: "com.diffplug.spotless",
                artifact_id: "spotless-maven-plugin",
                body: extract_block(QUALITY, 0),
            },
            Fragment {
                id: "maven-pmd-plugin",
                kind: FragmentKind::Plugin,
                group_id: "org.apache.maven.plugins",
                artifact_id: "maven-pmd-plugin",
                body: extract_block(QUALITY, 1),
            },
            Fragment {
                id: "spotbugs-maven-plugin",
                kind: FragmentKind::Plugin,
                group_id: "com.github.spotbugs",
                artifact_id: "spotbugs-maven-plugin",
                body: extract_block(QUALITY, 2),
            },
        ],
        ProfileArg::SmartDoc => vec![Fragment {
            id: "smart-doc-maven-plugin",
            kind: FragmentKind::Plugin,
            group_id: "com.ly.smart-doc",
            artifact_id: "smart-doc-maven-plugin",
            body: SMART_DOC,
        }],
        ProfileArg::Flyway => vec![
            Fragment {
                id: "flyway-core",
                kind: FragmentKind::Dependency,
                group_id: "org.flywaydb",
                artifact_id: "flyway-core",
                body: extract_block(FLYWAY, 0),
            },
            Fragment {
                id: "flyway-maven-plugin",
                kind: FragmentKind::Plugin,
                group_id: "org.flywaydb",
                artifact_id: "flyway-maven-plugin",
                body: extract_block(FLYWAY, 1),
            },
        ],
    }
}

/// Templates with multiple top-level elements are concatenated; this helper
/// returns one of them by index (0-based). It splits on the top-level closing
/// `</dependency>` or `</plugin>` boundary.
fn extract_block(template: &'static str, index: usize) -> &'static str {
    // Tally top-level open/close tags, then return the substring covering the Nth.
    let bytes = template.as_bytes();
    let mut blocks: Vec<(usize, usize)> = Vec::new();
    let mut depth: i32 = 0;
    let mut block_start: Option<usize> = None;
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'<' {
            // Skip XML declarations / comments / closing tags
            if i + 1 < bytes.len() && bytes[i + 1] == b'/' {
                // closing tag — find '>'
                if let Some(end) = template[i..].find('>').map(|e| i + e + 1) {
                    depth -= 1;
                    if depth == 0 {
                        if let Some(start) = block_start.take() {
                            blocks.push((start, end));
                        }
                    }
                    i = end;
                    continue;
                }
            }
            if i + 1 < bytes.len() && (bytes[i + 1] == b'!' || bytes[i + 1] == b'?') {
                if let Some(end) = template[i..].find('>').map(|e| i + e + 1) {
                    i = end;
                    continue;
                }
            }
            // Opening tag
            if let Some(end) = template[i..].find('>').map(|e| i + e + 1) {
                let self_closing = bytes[end - 2] == b'/';
                if depth == 0 {
                    block_start = Some(i);
                }
                if !self_closing {
                    depth += 1;
                } else if depth == 0 {
                    blocks.push((i, end));
                    block_start = None;
                }
                i = end;
                continue;
            }
        }
        i += 1;
    }
    let (s, e) = blocks
        .get(index)
        .copied()
        .unwrap_or((0, template.len()));
    &template[s..e]
}

#[derive(Serialize)]
pub struct FragmentEntry {
    pub id: String,
    pub present: bool,
    pub action: &'static str,
}

#[derive(Serialize)]
pub struct PomReport {
    pub profile: String,
    pub fragments: Vec<FragmentEntry>,
    pub missing: Vec<String>,
    pub already_present: Vec<String>,
    pub actions_taken: Vec<String>,
    pub ready: bool,
    pub blocking_errors: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

pub fn run(profile: ProfileArg, apply: bool, pom_path: &Path) -> Result<()> {
    let report = compute(profile, apply, pom_path)?;
    print_json(&report)
}

pub fn compute(profile: ProfileArg, apply: bool, pom_path: &Path) -> Result<PomReport> {
    if !pom_path.exists() {
        anyhow::bail!("pom.xml not found at {}", pom_path.display());
    }
    let original = read_pom(pom_path)?;
    let indent = detect_indent(&original);
    let fragments = profile_fragments(profile);

    let mut working = original.clone();
    let mut entries: Vec<FragmentEntry> = Vec::new();
    let mut missing: Vec<String> = Vec::new();
    let mut already_present: Vec<String> = Vec::new();
    let mut actions_taken: Vec<String> = Vec::new();
    let mut blocking_errors: Vec<String> = Vec::new();

    for frag in &fragments {
        let parent_kind_str = match frag.kind {
            FragmentKind::Dependency => "<dependencies>",
            FragmentKind::Plugin => "<build><plugins>",
        };

        // Locate parent; create if missing under apply.
        let parent_span = match frag.kind {
            FragmentKind::Dependency => {
                if dependencies_span(&working).is_none() {
                    if apply {
                        working = ensure_dependencies(&working, &indent)?;
                    } else {
                        // Dry-run: count as missing, no parent yet.
                        entries.push(FragmentEntry {
                            id: frag.id.to_string(),
                            present: false,
                            action: "reported",
                        });
                        missing.push(frag.id.to_string());
                        continue;
                    }
                }
                dependencies_span(&working)
            }
            FragmentKind::Plugin => {
                if plugins_span(&working).is_none() {
                    if apply {
                        working = ensure_plugins(&working, &indent)?;
                    } else {
                        entries.push(FragmentEntry {
                            id: frag.id.to_string(),
                            present: false,
                            action: "reported",
                        });
                        missing.push(frag.id.to_string());
                        continue;
                    }
                }
                plugins_span(&working)
            }
        };
        let span: ElementSpan = match parent_span {
            Some(s) => s,
            None => {
                blocking_errors
                    .push(format!("could not locate {} after ensure", parent_kind_str));
                continue;
            }
        };

        let present = is_fragment_present(&working, &span, frag);
        if present {
            entries.push(FragmentEntry {
                id: frag.id.to_string(),
                present: true,
                action: "skipped",
            });
            already_present.push(frag.id.to_string());
            continue;
        }

        if apply {
            // Re-detect indent (in case ensure_* changed structure) and the parent's
            // closing-indent: child indent is closing_indent + indent_unit.
            let child_indent = format!("{}{}", span.closing_indent, indent);
            let body = indent_fragment(frag.body, &child_indent, &indent);
            working = insert_into_span(&working, &span, &body);
            entries.push(FragmentEntry {
                id: frag.id.to_string(),
                present: false,
                action: "added",
            });
            actions_taken.push(format!(
                "added {} to {}",
                frag.id,
                match frag.kind {
                    FragmentKind::Dependency => "<dependencies>",
                    FragmentKind::Plugin => "<build><plugins>",
                }
            ));
        } else {
            entries.push(FragmentEntry {
                id: frag.id.to_string(),
                present: false,
                action: "reported",
            });
            missing.push(frag.id.to_string());
        }
    }

    if apply && working != original {
        write_pom(pom_path, &working).context("writing modified pom.xml")?;
    }

    let ready = missing.is_empty() && blocking_errors.is_empty();
    Ok(PomReport {
        profile: profile.name().to_string(),
        fragments: entries,
        missing,
        already_present,
        actions_taken,
        ready,
        blocking_errors,
        warnings: Vec::new(),
    })
}

fn is_fragment_present(text: &str, span: &ElementSpan, frag: &Fragment) -> bool {
    let child_tag = match frag.kind {
        FragmentKind::Dependency => "dependency",
        FragmentKind::Plugin => "plugin",
    };
    child_exists(text, span, child_tag, |body| {
        child_text(body, "groupId").as_deref() == Some(frag.group_id)
            && child_text(body, "artifactId").as_deref() == Some(frag.artifact_id)
    })
}
