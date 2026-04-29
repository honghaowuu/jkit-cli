//! `jkit drift check --plan <run>/plan.md` — pre-merge drift between
//! `<run>/proposed-api.yaml` (the immutable contract writing-plans
//! authored) and the current state of the code via smartdoc.
//!
//! ## Diff scope
//!
//! Per `(path, method)` declared in the proposal:
//!
//! - `endpoint_missing_in_code` — proposal lists it; code doesn't have it
//! - `endpoint_added_in_code`   — proposal touches the path; code has an
//!   extra method on the same path the proposal didn't declare
//! - `response_status_missing_in_code` / `response_status_added_in_code`
//! - `request_required_field_missing_in_code` / `..._added_in_code`
//!   — set difference over each operation's required request body fields
//!   (resolves `$ref` to `components.schemas`; recurses into `allOf`)
//! - `parameter_missing_in_code` / `parameter_added_in_code`
//!   — set difference over `(name, in)` pairs of declared parameters
//! - `parameter_required_changed` — same `(name, in)`, different `required` flag
//! - `response_required_field_missing_in_code` / `..._added_in_code`
//!   — set difference over each `responses.<status>` schema's required fields,
//!   restricted to status codes present in BOTH proposal and code (status-set
//!   diffs are caught separately by `response_status_*` and surfacing
//!   field-level diffs against an absent status would just duplicate them)
//!
//! Untouched paths in code (paths the proposal doesn't touch) intentionally
//! do not surface as drift — the proposal is a strict delta of what changes,
//! not a full surface.
//!
//! Type-level diffs (a property whose `type` changed between proposal and
//! code) are not emitted — schema validation annotations + runtime tests
//! usually catch those faster. `oneOf`/`anyOf` requireds aren't walked
//! because they're conditional, not absolute.

use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::{json, Value as JsonValue};
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
    pub ok: bool,
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
    /// Set for `--against published-contract` mode — `"issue"` (major bump
    /// required) or `"warning"` (minor bump). Unset for `--plan` mode where
    /// every drift entry already routes through the A/B/C gate uniformly.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub severity: Option<&'static str>,
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
    let proposal_yaml: YamlValue = serde_yaml::from_str(&proposal_text)
        .with_context(|| format!("parsing {} as YAML", proposal_path.display()))?;

    let code_spec = crate::migrate::smartdoc::run_smartdoc(pom_path)?;

    let drift = diff(&proposal_yaml, &code_spec);
    let ok = drift.is_empty();
    print_json(&CheckReport {
        ok,
        reason: None,
        run: run_label,
        drift,
    })
}

/// Pure diff between proposal (YAML) and code's smartdoc output (JSON).
/// YAML is converted to JSON internally so the schema-walking helpers stay
/// single-implementation.
pub fn diff(proposal: &YamlValue, code: &JsonValue) -> Vec<DriftFinding> {
    let proposal_json: JsonValue = serde_json::to_value(proposal).unwrap_or(JsonValue::Null);
    diff_inner(&proposal_json, code)
}

fn diff_inner(proposal: &JsonValue, code: &JsonValue) -> Vec<DriftFinding> {
    let proposed_paths = paths_from(proposal);
    let code_paths = paths_from(code);
    let proposal_components = proposal.get("components");
    let code_components = code.get("components");

    let mut out: Vec<DriftFinding> = Vec::new();

    for (path, proposed_methods) in &proposed_paths {
        let code_methods = code_paths.get(path);
        for (method, prop_op_raw) in proposed_methods {
            let endpoint = endpoint_label(method, path);
            let actual_op_raw = code_methods.and_then(|m| m.get(method));
            match actual_op_raw {
                None => out.push(DriftFinding {
                    kind: "endpoint_missing_in_code",
                    severity: None,
                    endpoint,
                    proposed: Some(json!({"declared": true})),
                    actual: None,
                }),
                Some(actual) => {
                    let prop_view = OperationView::from(prop_op_raw, proposal_components);
                    let actual_view = OperationView::from(actual, code_components);
                    diff_response_statuses(&endpoint, &prop_view, &actual_view, &mut out);
                    diff_request_required(&endpoint, &prop_view, &actual_view, &mut out);
                    diff_parameters(&endpoint, &prop_view, &actual_view, &mut out);
                    diff_response_required(&endpoint, &prop_view, &actual_view, &mut out);
                }
            }
        }
        if let Some(code_methods) = code_methods {
            for (method, _) in code_methods {
                if !proposed_methods.contains_key(method) {
                    out.push(DriftFinding {
                        kind: "endpoint_added_in_code",
                        severity: None,
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
    p: &OperationView,
    a: &OperationView,
    out: &mut Vec<DriftFinding>,
) {
    for status in p.response_codes.difference(&a.response_codes) {
        out.push(DriftFinding {
            kind: "response_status_missing_in_code",
            severity: None,
            endpoint: endpoint.to_string(),
            proposed: Some(JsonValue::String(status.clone())),
            actual: None,
        });
    }
    for status in a.response_codes.difference(&p.response_codes) {
        out.push(DriftFinding {
            kind: "response_status_added_in_code",
            severity: None,
            endpoint: endpoint.to_string(),
            proposed: None,
            actual: Some(JsonValue::String(status.clone())),
        });
    }
}

fn diff_request_required(
    endpoint: &str,
    p: &OperationView,
    a: &OperationView,
    out: &mut Vec<DriftFinding>,
) {
    for field in p.request_required.difference(&a.request_required) {
        out.push(DriftFinding {
            kind: "request_required_field_missing_in_code",
            severity: None,
            endpoint: endpoint.to_string(),
            proposed: Some(JsonValue::String(field.clone())),
            actual: None,
        });
    }
    for field in a.request_required.difference(&p.request_required) {
        out.push(DriftFinding {
            kind: "request_required_field_added_in_code",
            severity: None,
            endpoint: endpoint.to_string(),
            proposed: None,
            actual: Some(JsonValue::String(field.clone())),
        });
    }
}

fn diff_response_required(
    endpoint: &str,
    p: &OperationView,
    a: &OperationView,
    out: &mut Vec<DriftFinding>,
) {
    // Restrict to status codes present in both. Status-set diffs are caught
    // by diff_response_statuses; if status `201` is in proposal but absent
    // from code, surfacing every required-field difference under it would
    // just duplicate the status-level finding.
    for (status, prop_set) in &p.response_required {
        let Some(actual_set) = a.response_required.get(status) else {
            continue;
        };
        for field in prop_set.difference(actual_set) {
            out.push(DriftFinding {
                kind: "response_required_field_missing_in_code",
                severity: None,
                endpoint: endpoint.to_string(),
                proposed: Some(json!({"status": status, "field": field})),
                actual: None,
            });
        }
        for field in actual_set.difference(prop_set) {
            out.push(DriftFinding {
                kind: "response_required_field_added_in_code",
                severity: None,
                endpoint: endpoint.to_string(),
                proposed: None,
                actual: Some(json!({"status": status, "field": field})),
            });
        }
    }
}

fn diff_parameters(
    endpoint: &str,
    p: &OperationView,
    a: &OperationView,
    out: &mut Vec<DriftFinding>,
) {
    let p_keys: BTreeSet<_> = p.parameters.keys().collect();
    let a_keys: BTreeSet<_> = a.parameters.keys().collect();
    for key in p_keys.difference(&a_keys) {
        let required = p.parameters[*key];
        out.push(DriftFinding {
            kind: "parameter_missing_in_code",
            severity: None,
            endpoint: endpoint.to_string(),
            proposed: Some(json!({"name": key.0, "in": key.1, "required": required})),
            actual: None,
        });
    }
    for key in a_keys.difference(&p_keys) {
        let required = a.parameters[*key];
        out.push(DriftFinding {
            kind: "parameter_added_in_code",
            severity: None,
            endpoint: endpoint.to_string(),
            proposed: None,
            actual: Some(json!({"name": key.0, "in": key.1, "required": required})),
        });
    }
    for key in p_keys.intersection(&a_keys) {
        let pr = p.parameters[*key];
        let ar = a.parameters[*key];
        if pr != ar {
            out.push(DriftFinding {
                kind: "parameter_required_changed",
                severity: None,
                endpoint: endpoint.to_string(),
                proposed: Some(json!({"name": key.0, "in": key.1, "required": pr})),
                actual: Some(json!({"name": key.0, "in": key.1, "required": ar})),
            });
        }
    }
}

fn endpoint_label(method: &str, path: &str) -> String {
    format!("{} {}", method.to_uppercase(), path)
}

/// CLI entry for `--against published-contract --contract-yaml <path>`.
/// Loads the contract YAML, runs smartdoc, diffs, prints. The plan-mode
/// twin (`run`) lives above.
pub fn run_against_contract(contract_path: &Path, pom_path: &Path) -> Result<()> {
    let cwd = std::env::current_dir().context("reading cwd")?;
    let contract_abs = if contract_path.is_absolute() {
        contract_path.to_path_buf()
    } else {
        cwd.join(contract_path)
    };
    if !contract_abs.is_file() {
        anyhow::bail!("contract file not found: {}", contract_path.display());
    }
    let contract_text = fs::read_to_string(&contract_abs)
        .with_context(|| format!("reading {}", contract_abs.display()))?;
    let contract_yaml: YamlValue = serde_yaml::from_str(&contract_text)
        .with_context(|| format!("parsing {} as YAML", contract_abs.display()))?;

    let code_spec = crate::migrate::smartdoc::run_smartdoc(pom_path)?;

    let drift = diff_against_contract(&contract_yaml, &code_spec);
    let any_issue = drift.iter().any(|f| f.severity == Some("issue"));

    let label = contract_abs
        .strip_prefix(&cwd)
        .unwrap_or(&contract_abs)
        .to_string_lossy()
        .into_owned();

    print_json(&serde_json::json!({
        "ok": !any_issue,
        "mode": "against_published_contract",
        "contract": label,
        "drift": drift,
    }))
}

/// Bidirectional diff between a published `contract.yaml` (the last release's
/// authoritative surface) and current code's smartdoc output. Unlike the
/// proposal-vs-code diff, the contract is the FULL surface — every endpoint
/// in either side that doesn't have a counterpart is drift.
///
/// Annotates each finding with `severity` per the formal-docs redesign's
/// semver table:
///   additions / loosenings → "warning" (minor bump)
///   removals / tightenings / status changes → "issue" (major bump required)
///
/// `kind` strings are the same as `--plan` mode (`endpoint_missing_in_code`
/// = removed; `endpoint_added_in_code` = added; etc.) — consumers branch on
/// `severity` rather than maintaining two sets of finding type names.
pub fn diff_against_contract(contract: &YamlValue, code: &JsonValue) -> Vec<DriftFinding> {
    let contract_json: JsonValue = serde_json::to_value(contract).unwrap_or(JsonValue::Null);
    let mut findings = diff_inner(&contract_json, code);
    // diff_inner emits the proposal-side findings (paths the contract declares,
    // diff'd against code). For full-surface mode we ALSO need to walk paths
    // present only in code (additions). The existing paths_from(...) → for
    // path in proposal { ... if !proposed_methods.contains(method) → added }
    // already handles methods on shared paths; what's still missing is paths
    // present only in code (no entry in the contract at all).
    let contract_paths = paths_from(&contract_json);
    let code_paths = paths_from(code);
    for (path, methods) in &code_paths {
        if contract_paths.contains_key(path) {
            continue;
        }
        for method in methods.keys() {
            findings.push(DriftFinding {
                kind: "endpoint_added_in_code",
                severity: None,
                endpoint: endpoint_label(method, path),
                proposed: None,
                actual: Some(json!({"declared": true})),
            });
        }
    }
    annotate_severity_against_contract(&mut findings);
    findings
}

/// Walk findings and set `severity` per the spec table. Only meaningful in
/// the `--against published-contract` flow.
fn annotate_severity_against_contract(findings: &mut [DriftFinding]) {
    for f in findings.iter_mut() {
        f.severity = Some(severity_for(f));
    }
}

fn severity_for(f: &DriftFinding) -> &'static str {
    match f.kind {
        // Additions / loosenings — minor bump
        "endpoint_added_in_code"
        | "response_status_added_in_code"
        | "request_required_field_missing_in_code"
        | "response_required_field_added_in_code" => "warning",

        // Removals / tightenings — major bump
        "endpoint_missing_in_code"
        | "response_status_missing_in_code"
        | "request_required_field_added_in_code"
        | "response_required_field_missing_in_code" => "issue",

        "parameter_added_in_code" => {
            // Adding an optional parameter is a minor bump; adding a required
            // one breaks existing callers (they must now supply it).
            if param_required_actual(f) { "issue" } else { "warning" }
        }
        "parameter_missing_in_code" => {
            // Removing an optional parameter is broadly safe; removing a
            // required one means callers that supplied it now hit an
            // unexpected field — not technically breaking on the wire, but
            // a contract change consumers depend on.
            if param_required_proposed(f) { "issue" } else { "warning" }
        }
        "parameter_required_changed" => {
            // false → true tightens (issue); true → false loosens (warning).
            if param_required_proposed(f) {
                "warning"
            } else {
                "issue"
            }
        }

        // Unknown finding type — fail closed.
        _ => "issue",
    }
}

fn param_required_proposed(f: &DriftFinding) -> bool {
    f.proposed
        .as_ref()
        .and_then(|v| v.get("required"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

fn param_required_actual(f: &DriftFinding) -> bool {
    f.actual
        .as_ref()
        .and_then(|v| v.get("required"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

#[derive(Debug, Clone)]
struct OperationView {
    response_codes: BTreeSet<String>,
    /// Required fields drawn from the request body schema (set-union over
    /// content-types; resolves `$ref` and `allOf`).
    request_required: BTreeSet<String>,
    /// `(name, in)` → `required: bool` over declared parameters (resolves
    /// `$ref` to `#/components/parameters/<Name>`).
    parameters: BTreeMap<(String, String), bool>,
    /// Per-status-code required fields drawn from the response schemas
    /// (set-union over content-types; resolves `$ref` and `allOf`).
    response_required: BTreeMap<String, BTreeSet<String>>,
}

impl OperationView {
    fn from(op: &JsonValue, components: Option<&JsonValue>) -> Self {
        OperationView {
            response_codes: response_codes(op),
            request_required: request_required_fields(op, components),
            parameters: parameters_for(op, components),
            response_required: response_required_fields(op, components),
        }
    }
}

/// Convert either YAML mapping or JSON object into a path → method →
/// operation-Value tree. The serde_json::Value comes from either smartdoc
/// (already JSON) or the proposal (converted via `serde_json::to_value`).
fn paths_from(v: &JsonValue) -> BTreeMap<String, BTreeMap<String, JsonValue>> {
    let mut out: BTreeMap<String, BTreeMap<String, JsonValue>> = BTreeMap::new();
    let Some(paths) = v.get("paths").and_then(|p| p.as_object()) else {
        return out;
    };
    for (path, item) in paths {
        let Some(item_map) = item.as_object() else {
            continue;
        };
        let mut methods: BTreeMap<String, JsonValue> = BTreeMap::new();
        for (op_k, op_v) in item_map {
            if !HTTP_METHODS.contains(&op_k.as_str()) {
                continue;
            }
            methods.insert(op_k.clone(), op_v.clone());
        }
        if !methods.is_empty() {
            out.insert(path.clone(), methods);
        }
    }
    out
}

fn response_codes(op: &JsonValue) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let Some(map) = op.get("responses").and_then(|r| r.as_object()) else {
        return out;
    };
    for k in map.keys() {
        out.insert(k.clone());
    }
    out
}

/// Walk `operation.requestBody.content.<media>.schema` for each content
/// type, follow `$ref` into `components.schemas`, recurse into `allOf`,
/// union the `required: [...]` arrays. `oneOf`/`anyOf` are skipped — their
/// requireds are conditional, not absolute.
fn request_required_fields(op: &JsonValue, components: Option<&JsonValue>) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let Some(body) = op.get("requestBody") else {
        return out;
    };
    // A requestBody may itself be a $ref to components.requestBodies.
    let body = resolve_ref(body, components, "requestBodies").unwrap_or(body.clone());
    let Some(content) = body.get("content").and_then(|c| c.as_object()) else {
        return out;
    };
    let mut visited: BTreeSet<String> = BTreeSet::new();
    for (_mt, media) in content {
        if let Some(schema) = media.get("schema") {
            collect_required(schema, components, &mut out, &mut visited);
        }
    }
    out
}

/// Walk `operation.responses.<status>.content.<media>.schema` for each
/// status code; same `$ref` / `allOf` semantics as request bodies. Returns
/// a per-status map; status codes whose response object has no schema are
/// absent from the map (rather than mapping to an empty set) so the diff
/// caller can distinguish "no schema declared" from "schema declared with
/// no required fields."
fn response_required_fields(
    op: &JsonValue,
    components: Option<&JsonValue>,
) -> BTreeMap<String, BTreeSet<String>> {
    let mut out = BTreeMap::new();
    let Some(responses) = op.get("responses").and_then(|r| r.as_object()) else {
        return out;
    };
    for (status, response) in responses {
        let response = resolve_ref(response, components, "responses").unwrap_or(response.clone());
        let Some(content) = response.get("content").and_then(|c| c.as_object()) else {
            continue;
        };
        let mut required: BTreeSet<String> = BTreeSet::new();
        let mut visited: BTreeSet<String> = BTreeSet::new();
        let mut had_schema = false;
        for (_mt, media) in content {
            if let Some(schema) = media.get("schema") {
                had_schema = true;
                collect_required(schema, components, &mut required, &mut visited);
            }
        }
        if had_schema {
            out.insert(status.clone(), required);
        }
    }
    out
}

fn collect_required(
    schema: &JsonValue,
    components: Option<&JsonValue>,
    out: &mut BTreeSet<String>,
    visited: &mut BTreeSet<String>,
) {
    // Resolve $ref first.
    if let Some(reference) = schema.get("$ref").and_then(|r| r.as_str()) {
        if !visited.insert(reference.to_string()) {
            return;
        }
        if let Some(target) = resolve_components_path(reference, components) {
            collect_required(&target, components, out, visited);
        }
        return;
    }
    if let Some(arr) = schema.get("required").and_then(|r| r.as_array()) {
        for v in arr {
            if let Some(s) = v.as_str() {
                out.insert(s.to_string());
            }
        }
    }
    if let Some(all_of) = schema.get("allOf").and_then(|a| a.as_array()) {
        for branch in all_of {
            collect_required(branch, components, out, visited);
        }
    }
}

/// `(name, in)` → `required` for declared parameters. Resolves
/// `$ref` to `#/components/parameters/<Name>`. `path` parameters are
/// implicitly `required: true` per OpenAPI 3.x — coerced here.
fn parameters_for(
    op: &JsonValue,
    components: Option<&JsonValue>,
) -> BTreeMap<(String, String), bool> {
    let mut out = BTreeMap::new();
    let Some(arr) = op.get("parameters").and_then(|p| p.as_array()) else {
        return out;
    };
    for p in arr {
        let resolved = resolve_ref(p, components, "parameters").unwrap_or_else(|| p.clone());
        let name = resolved
            .get("name")
            .and_then(|n| n.as_str())
            .map(str::to_string);
        let in_ = resolved
            .get("in")
            .and_then(|n| n.as_str())
            .map(str::to_string);
        let required = match resolved.get("required") {
            Some(JsonValue::Bool(b)) => *b,
            _ => false,
        };
        if let (Some(n), Some(i)) = (name, in_) {
            // OpenAPI 3.x: path parameters MUST be required.
            let required = required || i == "path";
            out.insert((n, i), required);
        }
    }
    out
}

/// If `node` is a `{$ref: "#/components/<bucket>/<Name>"}`, resolve it.
/// Returns `None` for non-ref nodes; resolution failure also returns `None`
/// so the caller can fall through to its inline-shape branch.
fn resolve_ref(
    node: &JsonValue,
    components: Option<&JsonValue>,
    bucket: &str,
) -> Option<JsonValue> {
    let reference = node.get("$ref")?.as_str()?;
    let prefix = format!("#/components/{bucket}/");
    let name = reference.strip_prefix(&prefix)?;
    let comp = components?.get(bucket)?.as_object()?;
    comp.get(name).cloned()
}

/// Resolve a `#/components/<bucket>/<Name>` $ref string regardless of bucket
/// (used for nested schema $refs whose bucket name is fixed).
fn resolve_components_path(reference: &str, components: Option<&JsonValue>) -> Option<JsonValue> {
    let rest = reference.strip_prefix("#/components/")?;
    let mut parts = rest.splitn(2, '/');
    let bucket = parts.next()?;
    let name = parts.next()?;
    components?.get(bucket)?.get(name).cloned()
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
        assert!(d
            .iter()
            .any(|f| f.kind == "endpoint_missing_in_code" && f.endpoint == "POST /a"));
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
        assert!(d
            .iter()
            .any(|f| f.kind == "endpoint_added_in_code" && f.endpoint == "GET /a"));
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
        assert!(d
            .iter()
            .any(|f| f.kind == "response_status_missing_in_code"
                && f.endpoint == "POST /a"
                && f.proposed == Some(JsonValue::String("409".into()))));
        assert!(d
            .iter()
            .any(|f| f.kind == "response_status_added_in_code"
                && f.endpoint == "POST /a"
                && f.actual == Some(JsonValue::String("200".into()))));
    }

    #[test]
    fn untouched_paths_in_code_do_not_drift() {
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
        assert!(d.is_empty(), "got: {:?}", d);
    }

    // --- Schema-level diffs ---

    #[test]
    fn request_required_field_added_in_code() {
        let proposal = yaml(
            r#"openapi: 3.0.3
info: {title: x, version: '1'}
paths:
  /a:
    post:
      requestBody:
        content:
          application/json:
            schema:
              type: object
              required: [customerId]
      responses:
        '201': {description: ok}
"#,
        );
        let code = json(
            r#"{"paths":{"/a":{"post":{
                "requestBody":{"content":{"application/json":{"schema":{
                    "type":"object",
                    "required":["customerId","amount"]
                }}}},
                "responses":{"201":{"description":"ok"}}
            }}}}"#,
        );
        let d = diff(&proposal, &code);
        assert!(d.iter().any(|f| f.kind == "request_required_field_added_in_code"
            && f.actual == Some(JsonValue::String("amount".into()))));
        assert!(!d
            .iter()
            .any(|f| f.kind == "request_required_field_missing_in_code"));
    }

    #[test]
    fn request_required_resolves_ref_to_components_schemas() {
        let proposal = yaml(
            r#"openapi: 3.0.3
info: {title: x, version: '1'}
paths:
  /a:
    post:
      requestBody:
        content:
          application/json:
            schema: {$ref: '#/components/schemas/CreateInvoice'}
      responses:
        '201': {description: ok}
components:
  schemas:
    CreateInvoice:
      type: object
      required: [customerId, amount]
"#,
        );
        let code = json(
            r#"{"paths":{"/a":{"post":{
                "requestBody":{"content":{"application/json":{"schema":{
                    "type":"object","required":["customerId"]
                }}}},
                "responses":{"201":{"description":"ok"}}
            }}}}"#,
        );
        let d = diff(&proposal, &code);
        // Proposal: {customerId, amount}; code: {customerId} → amount missing in code.
        assert!(d.iter().any(|f| f.kind == "request_required_field_missing_in_code"
            && f.proposed == Some(JsonValue::String("amount".into()))));
    }

    #[test]
    fn request_required_unions_allof_branches() {
        let proposal = yaml(
            r#"openapi: 3.0.3
info: {title: x, version: '1'}
paths:
  /a:
    post:
      requestBody:
        content:
          application/json:
            schema:
              allOf:
                - $ref: '#/components/schemas/Base'
                - type: object
                  required: [extra]
      responses:
        '201': {description: ok}
components:
  schemas:
    Base:
      type: object
      required: [baseId]
"#,
        );
        let code = json(
            r#"{"paths":{"/a":{"post":{
                "requestBody":{"content":{"application/json":{"schema":{
                    "type":"object","required":["baseId"]
                }}}},
                "responses":{"201":{"description":"ok"}}
            }}}}"#,
        );
        let d = diff(&proposal, &code);
        assert!(d.iter().any(|f| f.kind == "request_required_field_missing_in_code"
            && f.proposed == Some(JsonValue::String("extra".into()))));
    }

    #[test]
    fn parameter_added_in_code() {
        let proposal = yaml(
            r#"openapi: 3.0.3
info: {title: x, version: '1'}
paths:
  /a:
    get:
      parameters:
        - {name: page, in: query, required: true}
      responses:
        '200': {description: ok}
"#,
        );
        let code = json(
            r#"{"paths":{"/a":{"get":{
                "parameters":[
                    {"name":"page","in":"query","required":true},
                    {"name":"size","in":"query","required":false}
                ],
                "responses":{"200":{"description":"ok"}}
            }}}}"#,
        );
        let d = diff(&proposal, &code);
        assert!(d.iter().any(|f| f.kind == "parameter_added_in_code"
            && f.actual == Some(json!({"name":"size","in":"query","required":false}))));
    }

    #[test]
    fn parameter_missing_in_code() {
        let proposal = yaml(
            r#"openapi: 3.0.3
info: {title: x, version: '1'}
paths:
  /a:
    get:
      parameters:
        - {name: page, in: query, required: true}
        - {name: size, in: query, required: false}
      responses:
        '200': {description: ok}
"#,
        );
        let code = json(
            r#"{"paths":{"/a":{"get":{
                "parameters":[{"name":"page","in":"query","required":true}],
                "responses":{"200":{"description":"ok"}}
            }}}}"#,
        );
        let d = diff(&proposal, &code);
        assert!(d.iter().any(|f| f.kind == "parameter_missing_in_code"
            && f.proposed == Some(json!({"name":"size","in":"query","required":false}))));
    }

    #[test]
    fn parameter_required_flag_change() {
        let proposal = yaml(
            r#"openapi: 3.0.3
info: {title: x, version: '1'}
paths:
  /a:
    get:
      parameters:
        - {name: page, in: query, required: true}
      responses:
        '200': {description: ok}
"#,
        );
        let code = json(
            r#"{"paths":{"/a":{"get":{
                "parameters":[{"name":"page","in":"query","required":false}],
                "responses":{"200":{"description":"ok"}}
            }}}}"#,
        );
        let d = diff(&proposal, &code);
        assert!(d
            .iter()
            .any(|f| f.kind == "parameter_required_changed" && f.endpoint == "GET /a"));
    }

    #[test]
    fn response_required_field_missing_in_code() {
        let proposal = yaml(
            r#"openapi: 3.0.3
info: {title: x, version: '1'}
paths:
  /a:
    get:
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema:
                type: object
                required: [id, name]
"#,
        );
        let code = json(
            r#"{"paths":{"/a":{"get":{"responses":{"200":{
                "description":"ok",
                "content":{"application/json":{"schema":{
                    "type":"object","required":["id"]
                }}}
            }}}}}}"#,
        );
        let d = diff(&proposal, &code);
        assert!(d.iter().any(|f| f.kind == "response_required_field_missing_in_code"
            && f.proposed == Some(json!({"status":"200","field":"name"}))));
    }

    #[test]
    fn response_required_field_added_in_code() {
        let proposal = yaml(
            r#"openapi: 3.0.3
info: {title: x, version: '1'}
paths:
  /a:
    get:
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema:
                type: object
                required: [id]
"#,
        );
        let code = json(
            r#"{"paths":{"/a":{"get":{"responses":{"200":{
                "description":"ok",
                "content":{"application/json":{"schema":{
                    "type":"object","required":["id","createdAt"]
                }}}
            }}}}}}"#,
        );
        let d = diff(&proposal, &code);
        assert!(d.iter().any(|f| f.kind == "response_required_field_added_in_code"
            && f.actual == Some(json!({"status":"200","field":"createdAt"}))));
    }

    #[test]
    fn response_required_resolves_ref_and_allof() {
        let proposal = yaml(
            r#"openapi: 3.0.3
info: {title: x, version: '1'}
paths:
  /a:
    get:
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema:
                allOf:
                  - $ref: '#/components/schemas/Base'
                  - type: object
                    required: [extra]
components:
  schemas:
    Base:
      type: object
      required: [baseId]
"#,
        );
        let code = json(
            r#"{"paths":{"/a":{"get":{"responses":{"200":{
                "description":"ok",
                "content":{"application/json":{"schema":{
                    "type":"object","required":["baseId"]
                }}}
            }}}}}}"#,
        );
        let d = diff(&proposal, &code);
        // Proposal requires {baseId, extra} on 200; code requires {baseId}.
        assert!(d.iter().any(|f| f.kind == "response_required_field_missing_in_code"
            && f.proposed == Some(json!({"status":"200","field":"extra"}))));
    }

    #[test]
    fn response_field_diff_skipped_for_status_only_on_one_side() {
        // When 201 is in proposal but not in code, the status-set diff
        // already emits response_status_missing_in_code. Field-level diffs
        // for 201 should NOT also fire — it would duplicate the signal.
        let proposal = yaml(
            r#"openapi: 3.0.3
info: {title: x, version: '1'}
paths:
  /a:
    post:
      responses:
        '201':
          description: created
          content:
            application/json:
              schema:
                type: object
                required: [id]
"#,
        );
        let code = json(
            r#"{"paths":{"/a":{"post":{"responses":{"200":{
                "description":"ok",
                "content":{"application/json":{"schema":{
                    "type":"object","required":["id","createdAt"]
                }}}
            }}}}}}"#,
        );
        let d = diff(&proposal, &code);
        // Status-set diff fires once each direction; no field-level findings.
        assert!(d.iter().any(|f| f.kind == "response_status_missing_in_code"));
        assert!(d.iter().any(|f| f.kind == "response_status_added_in_code"));
        assert!(!d
            .iter()
            .any(|f| f.kind.starts_with("response_required_field_")));
    }

    // --- diff_against_contract + severity ---

    #[test]
    fn against_contract_endpoint_added_in_code_is_warning() {
        let contract = yaml(
            r#"openapi: 3.0.3
info: {title: x, version: '1'}
paths:
  /a:
    get:
      responses:
        '200': {description: ok}
"#,
        );
        let code = json(
            r#"{"paths":{
                "/a":{"get":{"responses":{"200":{"description":"ok"}}}},
                "/b":{"get":{"responses":{"200":{"description":"ok"}}}}
            }}"#,
        );
        let d = diff_against_contract(&contract, &code);
        let f = d
            .iter()
            .find(|f| f.kind == "endpoint_added_in_code" && f.endpoint == "GET /b")
            .expect("missing endpoint_added_in_code");
        assert_eq!(f.severity, Some("warning"));
    }

    #[test]
    fn against_contract_endpoint_removed_is_issue() {
        let contract = yaml(
            r#"openapi: 3.0.3
info: {title: x, version: '1'}
paths:
  /a:
    get:
      responses:
        '200': {description: ok}
  /b:
    get:
      responses:
        '200': {description: ok}
"#,
        );
        let code = json(r#"{"paths":{"/a":{"get":{"responses":{"200":{"description":"ok"}}}}}}"#);
        let d = diff_against_contract(&contract, &code);
        let f = d
            .iter()
            .find(|f| f.kind == "endpoint_missing_in_code" && f.endpoint == "GET /b")
            .expect("missing endpoint_missing_in_code");
        assert_eq!(f.severity, Some("issue"));
    }

    #[test]
    fn against_contract_response_status_change_severity() {
        let contract = yaml(
            r#"openapi: 3.0.3
info: {title: x, version: '1'}
paths:
  /a:
    get:
      responses:
        '200': {description: ok}
"#,
        );
        let code = json(
            r#"{"paths":{"/a":{"get":{"responses":{
                "404":{"description":"not found"}
            }}}}}"#,
        );
        let d = diff_against_contract(&contract, &code);
        // 200 removed → issue; 404 added → warning
        let removed = d
            .iter()
            .find(|f| f.kind == "response_status_missing_in_code")
            .unwrap();
        assert_eq!(removed.severity, Some("issue"));
        let added = d
            .iter()
            .find(|f| f.kind == "response_status_added_in_code")
            .unwrap();
        assert_eq!(added.severity, Some("warning"));
    }

    #[test]
    fn against_contract_request_required_tightening_is_issue() {
        let contract = yaml(
            r#"openapi: 3.0.3
info: {title: x, version: '1'}
paths:
  /a:
    post:
      requestBody:
        content:
          application/json:
            schema: {type: object, required: [customerId]}
      responses:
        '201': {description: ok}
"#,
        );
        let code = json(
            r#"{"paths":{"/a":{"post":{
                "requestBody":{"content":{"application/json":{"schema":{
                    "type":"object","required":["customerId","amount"]
                }}}},
                "responses":{"201":{"description":"ok"}}
            }}}}"#,
        );
        let d = diff_against_contract(&contract, &code);
        let f = d
            .iter()
            .find(|f| f.kind == "request_required_field_added_in_code")
            .expect("missing tightening finding");
        assert_eq!(f.severity, Some("issue"));
    }

    #[test]
    fn against_contract_request_required_loosening_is_warning() {
        let contract = yaml(
            r#"openapi: 3.0.3
info: {title: x, version: '1'}
paths:
  /a:
    post:
      requestBody:
        content:
          application/json:
            schema: {type: object, required: [customerId, amount]}
      responses:
        '201': {description: ok}
"#,
        );
        let code = json(
            r#"{"paths":{"/a":{"post":{
                "requestBody":{"content":{"application/json":{"schema":{
                    "type":"object","required":["customerId"]
                }}}},
                "responses":{"201":{"description":"ok"}}
            }}}}"#,
        );
        let d = diff_against_contract(&contract, &code);
        let f = d
            .iter()
            .find(|f| f.kind == "request_required_field_missing_in_code")
            .expect("missing loosening finding");
        assert_eq!(f.severity, Some("warning"));
    }

    #[test]
    fn against_contract_response_required_removal_is_issue() {
        let contract = yaml(
            r#"openapi: 3.0.3
info: {title: x, version: '1'}
paths:
  /a:
    get:
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema: {type: object, required: [id, name]}
"#,
        );
        let code = json(
            r#"{"paths":{"/a":{"get":{"responses":{"200":{
                "description":"ok",
                "content":{"application/json":{"schema":{
                    "type":"object","required":["id"]
                }}}
            }}}}}}"#,
        );
        let d = diff_against_contract(&contract, &code);
        let f = d
            .iter()
            .find(|f| f.kind == "response_required_field_missing_in_code")
            .expect("missing finding");
        assert_eq!(f.severity, Some("issue"));
    }

    #[test]
    fn against_contract_required_param_added_is_issue_optional_is_warning() {
        let contract = yaml(
            r#"openapi: 3.0.3
info: {title: x, version: '1'}
paths:
  /a:
    get:
      responses:
        '200': {description: ok}
"#,
        );
        let code = json(
            r#"{"paths":{"/a":{"get":{
                "parameters":[
                    {"name":"required-one","in":"query","required":true},
                    {"name":"optional-one","in":"query","required":false}
                ],
                "responses":{"200":{"description":"ok"}}
            }}}}"#,
        );
        let d = diff_against_contract(&contract, &code);
        let req = d
            .iter()
            .find(|f| f.kind == "parameter_added_in_code"
                && f.actual.as_ref().unwrap()["name"] == "required-one")
            .unwrap();
        assert_eq!(req.severity, Some("issue"));
        let opt = d
            .iter()
            .find(|f| f.kind == "parameter_added_in_code"
                && f.actual.as_ref().unwrap()["name"] == "optional-one")
            .unwrap();
        assert_eq!(opt.severity, Some("warning"));
    }

    #[test]
    fn against_contract_param_required_flag_change_severity() {
        let contract = yaml(
            r#"openapi: 3.0.3
info: {title: x, version: '1'}
paths:
  /a:
    get:
      parameters:
        - {name: page, in: query, required: false}
      responses:
        '200': {description: ok}
  /b:
    get:
      parameters:
        - {name: page, in: query, required: true}
      responses:
        '200': {description: ok}
"#,
        );
        let code = json(
            r#"{"paths":{
                "/a":{"get":{
                    "parameters":[{"name":"page","in":"query","required":true}],
                    "responses":{"200":{"description":"ok"}}
                }},
                "/b":{"get":{
                    "parameters":[{"name":"page","in":"query","required":false}],
                    "responses":{"200":{"description":"ok"}}
                }}
            }}"#,
        );
        let d = diff_against_contract(&contract, &code);
        let tightening = d
            .iter()
            .find(|f| f.kind == "parameter_required_changed" && f.endpoint == "GET /a")
            .unwrap();
        assert_eq!(tightening.severity, Some("issue"));
        let loosening = d
            .iter()
            .find(|f| f.kind == "parameter_required_changed" && f.endpoint == "GET /b")
            .unwrap();
        assert_eq!(loosening.severity, Some("warning"));
    }

    #[test]
    fn plan_mode_findings_have_no_severity() {
        let proposal = yaml(
            r#"openapi: 3.0.3
info: {title: x, version: '1'}
paths:
  /a:
    post:
      responses:
        '201': {description: ok}
"#,
        );
        let code = json(r#"{"paths":{"/a":{"post":{"responses":{"200":{"description":"ok"}}}}}}"#);
        let d = diff(&proposal, &code);
        assert!(!d.is_empty());
        for f in &d {
            assert!(f.severity.is_none(), "plan mode should not emit severity: {:?}", f);
        }
    }

    #[test]
    fn path_parameters_treated_as_required_implicitly() {
        // OpenAPI 3.x: path parameters are required by spec. If the proposal
        // omits the `required: true` field but the code emits it, that's NOT
        // drift — both are required.
        let proposal = yaml(
            r#"openapi: 3.0.3
info: {title: x, version: '1'}
paths:
  /a/{id}:
    get:
      parameters:
        - {name: id, in: path}
      responses:
        '200': {description: ok}
"#,
        );
        let code = json(
            r#"{"paths":{"/a/{id}":{"get":{
                "parameters":[{"name":"id","in":"path","required":true}],
                "responses":{"200":{"description":"ok"}}
            }}}}"#,
        );
        assert!(diff(&proposal, &code).is_empty());
    }
}
