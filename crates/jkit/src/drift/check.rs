//! `jkit drift check --plan <run>/plan.md` — pre-merge drift between
//! `<run>/proposed-api.yaml` (the immutable contract writing-plans
//! authored) and the current state of the code via smartdoc.
//!
//! ## Diff scope this slice
//!
//! - Per `(path, method)` declared in the proposal:
//!     - `endpoint_missing_in_code` — proposal lists it; code doesn't have it
//!     - `endpoint_added_in_code`   — proposal touches the path but code has
//!       an additional method on the same path the proposal didn't declare
//!     - `response_status_missing_in_code` / `response_status_added_in_code`
//!
//! Schema-level diffs (request body fields, response shape, parameter
//! changes) are deferred to a later slice — the proposal still requires
//! them in `components.schemas`, but the diff doesn't compare structurally
//! yet.

use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::{json, Map, Value as JsonValue};
use serde_yaml::Value as YamlValue;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::proposed_api::validate::FILE_RELATIVE as PROPOSED_API_RELATIVE;
use crate::util::print_json;

const HTTP_METHODS: &[&str] = &[
    "get", "put", "post", "delete", "options", "head", "patch", "trace",
];

#[derive(Serialize)]
pub struct CheckReport {
    /// True when no drift findings exist (or the proposal is absent).
    /// Distinct from the envelope's `ok` (the command succeeded either way).
    pub ok: bool,
    /// Set when the proposal is absent — drift check is a no-op for runs
    /// that don't touch the public API.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<&'static str>,
    pub run: String,
    pub drift: Vec<DriftFinding>,
}

#[derive(Serialize, Debug, Clone)]
pub struct DriftFinding {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub endpoint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proposed: Option<JsonValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual: Option<JsonValue>,
}

pub fn run(plan_path: &Path, pom_path: &Path) -> Result<()> {
    let cwd = std::env::current_dir().context("reading cwd")?;
    let plan_abs = if plan_path.is_absolute() {
        plan_path.to_path_buf()
    } else {
        cwd.join(plan_path)
    };
    if !plan_abs.is_file() {
        anyhow::bail!("plan file not found: {}", plan_path.display());
    }
    let run_abs: PathBuf = plan_abs
        .parent()
        .ok_or_else(|| anyhow::anyhow!("plan path has no parent: {}", plan_path.display()))?
        .to_path_buf();

    let proposal_path = run_abs.join(PROPOSED_API_RELATIVE);
    let run_label = run_abs
        .strip_prefix(&cwd)
        .unwrap_or(&run_abs)
        .to_string_lossy()
        .into_owned();

    if !proposal_path.exists() {
        return print_json(&CheckReport {
            ok: true,
            reason: Some("no_proposal"),
            run: run_label,
            drift: Vec::new(),
        });
    }

    let proposal_text = fs::read_to_string(&proposal_path)
        .with_context(|| format!("reading {}", proposal_path.display()))?;
    let proposal: YamlValue = serde_yaml::from_str(&proposal_text)
        .with_context(|| format!("parsing {} as YAML", proposal_path.display()))?;

    let code_spec = crate::migrate::smartdoc::run_smartdoc(pom_path)?;

    let drift = diff(&proposal, &code_spec);
    let ok = drift.is_empty();
    print_json(&CheckReport {
        ok,
        reason: None,
        run: run_label,
        drift,
    })
}

/// Pure diff between the proposal (YAML Value) and code's OpenAPI spec
/// (JSON Value from smartdoc). Symmetric: returns findings that go in
/// either direction (`*_missing_in_code` and `*_added_in_code`).
pub fn diff(proposal: &YamlValue, code: &JsonValue) -> Vec<DriftFinding> {
    let proposed_paths = paths_from_yaml(proposal);
    let code_paths = paths_from_json(code);

    let mut out: Vec<DriftFinding> = Vec::new();

    for (path, proposed_methods) in &proposed_paths {
        let code_methods = code_paths.get(path);
        for (method, prop_op) in proposed_methods {
            let endpoint = endpoint_label(method, path);
            let actual_op = code_methods.and_then(|m| m.get(method));
            match actual_op {
                None => out.push(DriftFinding {
                    kind: "endpoint_missing_in_code",
                    endpoint,
                    proposed: Some(json!({"declared": true})),
                    actual: None,
                }),
                Some(actual) => {
                    diff_response_statuses(&endpoint, prop_op, actual, &mut out);
                }
            }
        }
        // Methods on this path in code that the proposal didn't declare.
        if let Some(code_methods) = code_methods {
            for (method, _) in code_methods {
                if !proposed_methods.contains_key(method) {
                    out.push(DriftFinding {
                        kind: "endpoint_added_in_code",
                        endpoint: endpoint_label(method, path),
                        proposed: None,
                        actual: Some(json!({"declared": true})),
                    });
                }
            }
        }
    }
    out
}

fn diff_response_statuses(
    endpoint: &str,
    proposed_op: &OperationView,
    actual_op: &OperationView,
    out: &mut Vec<DriftFinding>,
) {
    for status in proposed_op.response_codes.difference(&actual_op.response_codes) {
        out.push(DriftFinding {
            kind: "response_status_missing_in_code",
            endpoint: endpoint.to_string(),
            proposed: Some(JsonValue::String(status.clone())),
            actual: None,
        });
    }
    for status in actual_op.response_codes.difference(&proposed_op.response_codes) {
        out.push(DriftFinding {
            kind: "response_status_added_in_code",
            endpoint: endpoint.to_string(),
            proposed: None,
            actual: Some(JsonValue::String(status.clone())),
        });
    }
}

fn endpoint_label(method: &str, path: &str) -> String {
    format!("{} {}", method.to_uppercase(), path)
}

#[derive(Debug, Clone)]
struct OperationView {
    response_codes: BTreeSet<String>,
}

fn empty_operation() -> OperationView {
    OperationView {
        response_codes: BTreeSet::new(),
    }
}

fn paths_from_yaml(v: &YamlValue) -> BTreeMap<String, BTreeMap<String, OperationView>> {
    let mut out = BTreeMap::new();
    let paths = match v.get("paths") {
        Some(YamlValue::Mapping(m)) => m,
        _ => return out,
    };
    for (path_key, path_item) in paths {
        let path = match path_key.as_str() {
            Some(s) => s.to_string(),
            None => continue,
        };
        let item_map = match path_item {
            YamlValue::Mapping(m) => m,
            _ => continue,
        };
        let mut methods: BTreeMap<String, OperationView> = BTreeMap::new();
        for (op_k, op_v) in item_map {
            let method = match op_k.as_str() {
                Some(s) => s,
                None => continue,
            };
            if !HTTP_METHODS.contains(&method) {
                continue;
            }
            let view = OperationView {
                response_codes: yaml_response_codes(op_v),
            };
            methods.insert(method.to_string(), view);
        }
        if !methods.is_empty() {
            out.insert(path, methods);
        }
    }
    out
}

fn paths_from_json(v: &JsonValue) -> BTreeMap<String, BTreeMap<String, OperationView>> {
    let mut out = BTreeMap::new();
    let paths = match v.get("paths").and_then(|p| p.as_object()) {
        Some(p) => p,
        None => return out,
    };
    for (path, path_item) in paths {
        let item_map = match path_item.as_object() {
            Some(m) => m,
            None => continue,
        };
        let mut methods: BTreeMap<String, OperationView> = BTreeMap::new();
        for (op_k, op_v) in item_map {
            if !HTTP_METHODS.contains(&op_k.as_str()) {
                continue;
            }
            let view = OperationView {
                response_codes: json_response_codes(op_v),
            };
            methods.insert(op_k.clone(), view);
        }
        if !methods.is_empty() {
            out.insert(path.clone(), methods);
        }
    }
    out
}

fn yaml_response_codes(op: &YamlValue) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let responses = match op.get("responses") {
        Some(YamlValue::Mapping(m)) => m,
        _ => return out,
    };
    for (k, _) in responses {
        if let Some(s) = k.as_str() {
            out.insert(s.to_string());
        } else if let Some(n) = k.as_i64() {
            out.insert(n.to_string());
        }
    }
    out
}

fn json_response_codes(op: &JsonValue) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let Some(map) = op.get("responses").and_then(|r| r.as_object()) else {
        return out;
    };
    for (k, _) in map {
        out.insert(k.clone());
    }
    out
}

/// Suppress unused warnings for the empty-operation helper kept for the
/// later schema-diff slice.
#[allow(dead_code)]
fn _touch_helpers() {
    let _ = empty_operation();
    let _ = Map::<String, JsonValue>::new();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn yaml(s: &str) -> YamlValue {
        serde_yaml::from_str(s).unwrap()
    }
    fn json(s: &str) -> JsonValue {
        serde_json::from_str(s).unwrap()
    }

    #[test]
    fn no_drift_when_proposal_matches_code() {
        let proposal = yaml(
            r#"openapi: 3.0.3
info: {title: x, version: '1'}
paths:
  /a:
    post:
      responses:
        '201': {description: created}
        '401': {description: unauthorized}
"#,
        );
        let code = json(
            r#"{"openapi":"3.0.3","paths":{"/a":{"post":{"responses":{"201":{"description":"x"},"401":{"description":"y"}}}}}}"#,
        );
        assert!(diff(&proposal, &code).is_empty());
    }

    #[test]
    fn endpoint_missing_in_code() {
        let proposal = yaml(
            r#"openapi: 3.0.3
info: {title: x, version: '1'}
paths:
  /a:
    post:
      responses:
        '201': {description: created}
"#,
        );
        let code = json(r#"{"paths":{"/a":{"get":{"responses":{"200":{"description":"ok"}}}}}}"#);
        let d = diff(&proposal, &code);
        assert!(d.iter().any(|f| f.kind == "endpoint_missing_in_code" && f.endpoint == "POST /a"));
    }

    #[test]
    fn endpoint_added_in_code() {
        let proposal = yaml(
            r#"openapi: 3.0.3
info: {title: x, version: '1'}
paths:
  /a:
    post:
      responses:
        '201': {description: created}
"#,
        );
        let code = json(
            r#"{"paths":{"/a":{
                "post":{"responses":{"201":{"description":"created"}}},
                "get":{"responses":{"200":{"description":"ok"}}}
            }}}"#,
        );
        let d = diff(&proposal, &code);
        assert!(d.iter().any(|f| f.kind == "endpoint_added_in_code" && f.endpoint == "GET /a"));
    }

    #[test]
    fn response_status_mismatch_both_directions() {
        let proposal = yaml(
            r#"openapi: 3.0.3
info: {title: x, version: '1'}
paths:
  /a:
    post:
      responses:
        '201': {description: created}
        '409': {description: conflict}
"#,
        );
        let code = json(
            r#"{"paths":{"/a":{"post":{"responses":{
                "200":{"description":"ok-not-created"},
                "201":{"description":"created"}
            }}}}}"#,
        );
        let d = diff(&proposal, &code);
        // 409 is in proposal but not code
        assert!(d
            .iter()
            .any(|f| f.kind == "response_status_missing_in_code"
                && f.endpoint == "POST /a"
                && f.proposed == Some(JsonValue::String("409".into()))));
        // 200 is in code but not proposal
        assert!(d
            .iter()
            .any(|f| f.kind == "response_status_added_in_code"
                && f.endpoint == "POST /a"
                && f.actual == Some(JsonValue::String("200".into()))));
    }

    #[test]
    fn untouched_paths_in_code_do_not_drift() {
        // Proposal touches /a only. Code also has /b. /b should NOT appear
        // in drift — the proposal explicitly only declares paths it changes.
        let proposal = yaml(
            r#"openapi: 3.0.3
info: {title: x, version: '1'}
paths:
  /a:
    post:
      responses:
        '201': {description: created}
"#,
        );
        let code = json(
            r#"{"paths":{
                "/a":{"post":{"responses":{"201":{"description":"created"}}}},
                "/b":{"get":{"responses":{"200":{"description":"ok"}}}}
            }}"#,
        );
        let d = diff(&proposal, &code);
        assert!(d.iter().all(|f| !f.endpoint.contains("/b")), "got: {:?}", d);
    }

    #[test]
    fn proposal_without_paths_returns_no_drift() {
        let proposal = yaml(
            r#"openapi: 3.0.3
info: {title: x, version: '1'}
components:
  schemas:
    Foo: {type: object}
"#,
        );
        let code = json(r#"{"paths":{"/a":{"get":{"responses":{"200":{"description":"ok"}}}}}}"#);
        let d = diff(&proposal, &code);
        // schema-level diffs aren't implemented this slice; paths-only diff
        // sees no proposed paths → no drift.
        assert!(d.is_empty(), "got: {:?}", d);
    }
}
