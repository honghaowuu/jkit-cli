use anyhow::Result;
use std::collections::HashMap;
use std::path::Path;

use crate::contract::service_meta::Controller;
use crate::migrate::parse::{self, ClassDecl, Invocation, JavaFile};

#[derive(Debug)]
pub struct ControllerEntry {
    pub class_simple: String,
    pub methods: Vec<MethodEntry>,
}

#[derive(Debug)]
pub struct MethodEntry {
    pub name: String,
    pub http: Option<String>,
    pub path: Option<String>,
    pub javadoc_first_line: Option<String>,
    pub calls: Vec<CallEntry>,
}

#[derive(Debug)]
pub struct CallEntry {
    pub display: String,
    pub children: Vec<CallEntry>,
}

/// Returns `slug → [controllers]` populated with their resolved call graphs.
/// Slugs absent from the result either had no controller in the resolved
/// `class_to_slug` map or no class found in the parsed source — caller falls
/// back to the regex-rendered impl-logic for them.
pub fn build(
    src_root: &Path,
    controllers: &[Controller],
    class_to_slug: &HashMap<String, String>,
) -> Result<HashMap<String, Vec<ControllerEntry>>> {
    let files = parse::parse_dir(src_root)?;
    let class_idx = parse::build_class_index(&files);
    let by_fqn = build_fqn_map(&files);

    let mut by_slug: HashMap<String, Vec<ControllerEntry>> = HashMap::new();

    for c in controllers {
        let Some(slug) = class_to_slug.get(&c.class) else {
            continue;
        };
        let Some(parsed) = by_fqn.get(&c.class) else {
            continue;
        };

        let mut methods = Vec::new();
        for m in &c.methods {
            let parsed_m = parsed.methods.iter().find(|pm| pm.name == m.name);
            let calls = parsed_m
                .map(|pm| {
                    resolve_calls(&pm.invocations, parsed, &class_idx, &by_fqn, 1)
                })
                .unwrap_or_default();
            let javadoc_first_line = m
                .javadoc_text
                .as_ref()
                .and_then(|t| t.lines().next().map(|s| s.trim().to_string()))
                .filter(|s| !s.is_empty());
            methods.push(MethodEntry {
                name: m.name.clone(),
                http: m.http_method.clone(),
                path: m.path.clone(),
                javadoc_first_line,
                calls,
            });
        }

        by_slug
            .entry(slug.clone())
            .or_default()
            .push(ControllerEntry {
                class_simple: parsed.simple_name.clone(),
                methods,
            });
    }

    Ok(by_slug)
}

fn build_fqn_map(files: &[JavaFile]) -> HashMap<String, ClassDecl> {
    let mut out = HashMap::new();
    for f in files {
        for c in parse::extract_classes(f) {
            out.insert(c.fqn.clone(), c);
        }
    }
    out
}

fn resolve_calls(
    invs: &[Invocation],
    enclosing: &ClassDecl,
    idx: &HashMap<String, String>,
    by_fqn: &HashMap<String, ClassDecl>,
    max_depth: usize,
) -> Vec<CallEntry> {
    let mut out = Vec::new();
    for inv in invs {
        // Filter: skip super(...) / this(...) constructor delegations.
        if inv.method == "super" || inv.method == "this" {
            continue;
        }

        let resolved_fqn = inv.receiver.as_deref().and_then(|r| {
            enclosing
                .fields
                .iter()
                .find(|f| f.name == r)
                .and_then(|f| idx.get(parse::strip_generics(&f.type_text)).cloned())
        });

        let display = match (&inv.receiver, &resolved_fqn) {
            (Some(r), Some(fqn)) => {
                let simple = fqn.rsplit('.').next().unwrap_or(fqn);
                format!(
                    "{simple}.{} ({} {} via {r})",
                    inv.method,
                    inv.arg_count,
                    if inv.arg_count == 1 { "arg" } else { "args" },
                )
            }
            (Some(r), None) => format!(
                "{r}.{} ({} {}, unresolved)",
                inv.method,
                inv.arg_count,
                if inv.arg_count == 1 { "arg" } else { "args" },
            ),
            (None, _) => format!(
                "{} ({} {}, no receiver)",
                inv.method,
                inv.arg_count,
                if inv.arg_count == 1 { "arg" } else { "args" },
            ),
        };

        let children = if max_depth > 0 {
            resolved_fqn
                .as_ref()
                .and_then(|fqn| by_fqn.get(fqn))
                .and_then(|target| target.methods.iter().find(|m| m.name == inv.method).map(|m| (target, m)))
                .map(|(target_class, target_method)| {
                    resolve_calls(
                        &target_method.invocations,
                        target_class,
                        idx,
                        by_fqn,
                        max_depth - 1,
                    )
                })
                .unwrap_or_default()
        } else {
            Vec::new()
        };

        out.push(CallEntry { display, children });
    }
    out
}

pub fn render(slug: &str, entries: &[ControllerEntry]) -> String {
    let mut s = format!(
        "# {slug} implementation logic\n\
         \n\
         <!-- Auto-scaffolded by `jkit migrate scaffold-docs --use-call-graph` from controller→service call sites (2 levels deep). The structure below comes from the code; the **rules** are what /spec-delta should refine — the TODO markers are intentional. Chained calls (e.g. `repo.findById(...).orElseThrow()`) appear as separate entries; treat them as informational. -->\n\
         \n"
    );
    for entry in entries {
        for m in &entry.methods {
            let endpoint = match (&m.http, &m.path) {
                (Some(h), Some(p)) => format!("`{h} {p}`"),
                _ => "non-routed".to_string(),
            };
            s.push_str(&format!(
                "## {}#{} ({endpoint})\n\n",
                entry.class_simple, m.name
            ));
            if let Some(jd) = &m.javadoc_first_line {
                s.push_str(&format!("> {jd}\n\n"));
            }
            if m.calls.is_empty() {
                s.push_str("No service calls detected. <!-- TODO: business rule -->\n\n");
                continue;
            }
            s.push_str("Calls:\n\n");
            for call in &m.calls {
                render_call(&mut s, call, 0);
            }
            s.push('\n');
        }
    }
    s
}

fn render_call(s: &mut String, c: &CallEntry, indent: usize) {
    let pad = "  ".repeat(indent);
    s.push_str(&format!(
        "{pad}- `{}` — <!-- TODO: rule -->\n",
        c.display
    ));
    for child in &c.children {
        render_call(s, child, indent + 1);
    }
}
