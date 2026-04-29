use anyhow::{Context, Result};
use serde::Serialize;
use serde_yaml::{Mapping, Value};
use std::fs;
use std::path::Path;

use crate::util::print_json;

pub const FILE_RELATIVE: &str = "proposed-api.yaml";

#[derive(Serialize)]
pub struct ValidateReport {
    /// `true` when the proposal validates cleanly (no `severity: issue`
    /// findings) or when the file is absent. Distinct from the envelope's
    /// `ok` (the command itself succeeded — pass or fail).
    pub valid: bool,
    /// `true` when the file is absent under `<run>/proposed-api.yaml` —
    /// signals "this run doesn't touch the public API," not an error.
    pub absent: bool,
    pub path: String,
    /// Counts a contributor / drift check can sanity-check at a glance.
    pub paths_count: usize,
    pub schemas_count: usize,
    pub findings: Vec<Finding>,
}

#[derive(Serialize, Debug)]
pub struct Finding {
    pub code: &'static str,
    pub severity: &'static str, // "issue" | "warning"
    pub message: String,
    pub remediation: String,
}

pub fn run(run_dir: &Path) -> Result<()> {
    let cwd = std::env::current_dir().context("reading cwd")?;
    let run_abs = if run_dir.is_absolute() {
        run_dir.to_path_buf()
    } else {
        cwd.join(run_dir)
    };
    if !run_abs.is_dir() {
        anyhow::bail!("run dir not found: {}", run_dir.display());
    }
    let report = validate(&run_abs)?;
    print_json(&report)
}

pub fn validate(run_abs: &Path) -> Result<ValidateReport> {
    let path = run_abs.join(FILE_RELATIVE);
    let path_str = path.display().to_string();

    if !path.exists() {
        return Ok(ValidateReport {
            valid: true,
            absent: true,
            path: path_str,
            paths_count: 0,
            schemas_count: 0,
            findings: Vec::new(),
        });
    }

    let text = fs::read_to_string(&path)
        .with_context(|| format!("reading {}", path.display()))?;
    let parsed: Value = match serde_yaml::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            return Ok(ValidateReport {
                valid: false,
                absent: false,
                path: path_str,
                paths_count: 0,
                schemas_count: 0,
                findings: vec![Finding {
                    code: "malformed_yaml",
                    severity: "issue",
                    message: format!("YAML parse error: {e}"),
                    remediation: "Hand-fix the YAML; re-run validate-proposal.".to_string(),
                }],
            });
        }
    };

    let mut findings: Vec<Finding> = Vec::new();
    let top = match parsed {
        Value::Mapping(m) => m,
        _ => {
            findings.push(Finding {
                code: "not_a_mapping",
                severity: "issue",
                message: "top level must be a YAML mapping".into(),
                remediation: "Wrap the contents in a mapping (`key: value` form).".into(),
            });
            return Ok(ValidateReport {
                valid: false,
                absent: false,
                path: path_str,
                paths_count: 0,
                schemas_count: 0,
                findings,
            });
        }
    };

    check_openapi(&top, &mut findings);
    check_info(&top, &mut findings);
    let paths_count = check_paths(&top, &mut findings);
    let schemas_count = check_components(&top, &mut findings);

    if paths_count == 0 && schemas_count == 0 {
        findings.push(Finding {
            code: "empty_proposal",
            severity: "issue",
            message: "neither `paths` nor `components.schemas` contains entries — proposed-api.yaml must describe at least one added/modified path or schema, or be omitted entirely.".into(),
            remediation: "Either fill in the relevant section, or delete the file (drift check no-ops on absence).".into(),
        });
    }

    let valid = !findings.iter().any(|f| f.severity == "issue");
    Ok(ValidateReport {
        valid,
        absent: false,
        path: path_str,
        paths_count,
        schemas_count,
        findings,
    })
}

fn check_openapi(top: &Mapping, findings: &mut Vec<Finding>) {
    match top.get(Value::String("openapi".into())) {
        None => findings.push(Finding {
            code: "missing_openapi",
            severity: "issue",
            message: "missing required `openapi` field".into(),
            remediation: "Add `openapi: 3.0.3` (or another `3.x.y` version) at the top.".into(),
        }),
        Some(Value::String(v)) if v.starts_with("3.") => {}
        Some(other) => findings.push(Finding {
            code: "openapi_not_3x",
            severity: "issue",
            message: format!(
                "`openapi` must be a string starting with `3.`; got {:?}",
                other
            ),
            remediation: "Use `openapi: 3.0.3` (or `3.1.0`).".into(),
        }),
    }
}

fn check_info(top: &Mapping, findings: &mut Vec<Finding>) {
    let info = match top.get(Value::String("info".into())) {
        Some(Value::Mapping(m)) => m,
        Some(_) => {
            findings.push(Finding {
                code: "info_not_mapping",
                severity: "issue",
                message: "`info` must be a mapping".into(),
                remediation: "Use `info: {title: ..., version: ...}`.".into(),
            });
            return;
        }
        None => {
            findings.push(Finding {
                code: "missing_info",
                severity: "issue",
                message: "missing required `info` field".into(),
                remediation: "Add `info: {title: <name>, version: <semver>}`.".into(),
            });
            return;
        }
    };
    for required in ["title", "version"] {
        if !info.contains_key(Value::String(required.into())) {
            findings.push(Finding {
                code: "info_missing_field",
                severity: "issue",
                message: format!("`info.{required}` is required"),
                remediation: format!("Add `{required}: <value>` under `info`."),
            });
        }
    }
}

/// Returns the number of paths declared. Walks each operation looking for
/// security blocks and surfaces shape issues.
fn check_paths(top: &Mapping, findings: &mut Vec<Finding>) -> usize {
    let paths = match top.get(Value::String("paths".into())) {
        Some(Value::Mapping(m)) => m,
        Some(Value::Null) | None => return 0,
        Some(_) => {
            findings.push(Finding {
                code: "paths_not_mapping",
                severity: "issue",
                message: "`paths` must be a mapping of path templates to path-item objects".into(),
                remediation: "Use `paths: {/api/...: {get: {...}}}`.".into(),
            });
            return 0;
        }
    };

    const HTTP_METHODS: &[&str] = &[
        "get", "put", "post", "delete", "options", "head", "patch", "trace",
    ];

    let mut count = 0usize;
    for (path_key, item) in paths {
        let path_name = path_key.as_str().unwrap_or("<non-string>");
        if !path_name.starts_with('/') {
            findings.push(Finding {
                code: "path_not_rooted",
                severity: "warning",
                message: format!("path `{path_name}` does not start with `/`"),
                remediation: "Path templates must start with `/` per OpenAPI 3.x.".into(),
            });
        }
        let item_map = match item {
            Value::Mapping(m) => m,
            _ => {
                findings.push(Finding {
                    code: "path_item_not_mapping",
                    severity: "issue",
                    message: format!("path `{path_name}` is not a mapping"),
                    remediation: "Each path must hold a path-item object (HTTP-method keys).".into(),
                });
                continue;
            }
        };
        let mut had_op = false;
        for (op_key, op_val) in item_map {
            let op_name = match op_key.as_str() {
                Some(s) => s,
                None => continue,
            };
            if !HTTP_METHODS.contains(&op_name) {
                continue;
            }
            had_op = true;
            check_operation(path_name, op_name, op_val, findings);
        }
        if had_op {
            count += 1;
        }
    }
    count
}

fn check_operation(path: &str, method: &str, op_val: &Value, findings: &mut Vec<Finding>) {
    let op = match op_val {
        Value::Mapping(m) => m,
        _ => {
            findings.push(Finding {
                code: "operation_not_mapping",
                severity: "issue",
                message: format!("{} {path}: operation must be a mapping", method.to_uppercase()),
                remediation: "Use `<method>: {responses: {...}}`.".into(),
            });
            return;
        }
    };

    if !op.contains_key(Value::String("responses".into())) {
        findings.push(Finding {
            code: "operation_missing_responses",
            severity: "issue",
            message: format!(
                "{} {path}: operation has no `responses` block",
                method.to_uppercase()
            ),
            remediation: "Add at least one response (e.g. `responses: {'200': {description: ok}}`).".into(),
        });
    }

    if let Some(security) = op.get(Value::String("security".into())) {
        check_security(path, method, security, findings);
    }
}

fn check_security(path: &str, method: &str, sec: &Value, findings: &mut Vec<Finding>) {
    let entries = match sec {
        Value::Sequence(s) => s,
        _ => {
            findings.push(Finding {
                code: "security_not_list",
                severity: "issue",
                message: format!(
                    "{} {path}: `security` must be a list of mappings",
                    method.to_uppercase()
                ),
                remediation: "Use `security: [{permission: ['<CODE>']}]`.".into(),
            });
            return;
        }
    };
    for entry in entries {
        match entry {
            Value::Mapping(m) => {
                for (_, v) in m {
                    if !matches!(v, Value::Sequence(_)) {
                        findings.push(Finding {
                            code: "security_value_not_list",
                            severity: "issue",
                            message: format!(
                                "{} {path}: each `security` scheme entry must map to a list of strings (permission codes / scopes)",
                                method.to_uppercase()
                            ),
                            remediation: "Use `security: [{<scheme>: ['<code>']}]`.".into(),
                        });
                    }
                }
            }
            _ => findings.push(Finding {
                code: "security_entry_not_mapping",
                severity: "issue",
                message: format!(
                    "{} {path}: `security` list entries must be mappings",
                    method.to_uppercase()
                ),
                remediation: "Use `security: [{<scheme>: ['<code>']}]`.".into(),
            }),
        }
    }
}

/// Returns the count of `components.schemas` entries.
fn check_components(top: &Mapping, findings: &mut Vec<Finding>) -> usize {
    let components = match top.get(Value::String("components".into())) {
        Some(Value::Mapping(m)) => m,
        Some(Value::Null) | None => return 0,
        Some(_) => {
            findings.push(Finding {
                code: "components_not_mapping",
                severity: "issue",
                message: "`components` must be a mapping".into(),
                remediation: "Use `components: {schemas: {...}}`.".into(),
            });
            return 0;
        }
    };
    let mut count = 0usize;
    if let Some(schemas) = components.get(Value::String("schemas".into())) {
        match schemas {
            Value::Mapping(m) => count = m.len(),
            Value::Null => {}
            _ => findings.push(Finding {
                code: "schemas_not_mapping",
                severity: "issue",
                message: "`components.schemas` must be a mapping of schema name to definition".into(),
                remediation: "Use `components: {schemas: {Foo: {type: object, ...}}}`.".into(),
            }),
        }
    }
    if let Some(schemes) = components.get(Value::String("securitySchemes".into())) {
        if !matches!(schemes, Value::Mapping(_) | Value::Null) {
            findings.push(Finding {
                code: "security_schemes_not_mapping",
                severity: "issue",
                message: "`components.securitySchemes` must be a mapping".into(),
                remediation: "Use `components: {securitySchemes: {<name>: {...}}}`.".into(),
            });
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn write_proposal(run: &Path, body: &str) {
        let p = run.join(FILE_RELATIVE);
        std::fs::write(&p, body).unwrap();
    }

    #[test]
    fn absent_file_returns_ok_absent_true() {
        let tmp = tempdir().unwrap();
        let r = validate(tmp.path()).unwrap();
        assert!(r.valid);
        assert!(r.absent);
        assert!(r.findings.is_empty());
    }

    #[test]
    fn malformed_yaml_is_issue() {
        let tmp = tempdir().unwrap();
        write_proposal(tmp.path(), ":\n  - not-a-mapping\n");
        let r = validate(tmp.path()).unwrap();
        assert!(!r.valid);
        assert!(r.findings.iter().any(|f| f.code == "malformed_yaml"));
    }

    #[test]
    fn missing_openapi_and_info_are_issues() {
        let tmp = tempdir().unwrap();
        write_proposal(
            tmp.path(),
            "paths:\n  /a:\n    get:\n      responses:\n        '200': {description: ok}\n",
        );
        let r = validate(tmp.path()).unwrap();
        assert!(!r.valid);
        assert!(r.findings.iter().any(|f| f.code == "missing_openapi"));
        assert!(r.findings.iter().any(|f| f.code == "missing_info"));
    }

    #[test]
    fn happy_path_with_paths_only() {
        let tmp = tempdir().unwrap();
        write_proposal(
            tmp.path(),
            r#"openapi: 3.0.3
info:
  title: order-service
  version: '1.4.0'
paths:
  /api/billing/invoices:
    post:
      responses:
        '201': {description: created}
        '401': {description: unauthorized}
      security:
        - permission: ['124012.MANAGE']
"#,
        );
        let r = validate(tmp.path()).unwrap();
        assert!(r.valid, "findings: {:?}", r.findings);
        assert_eq!(r.paths_count, 1);
        assert_eq!(r.schemas_count, 0);
    }

    #[test]
    fn happy_path_with_schemas_only() {
        let tmp = tempdir().unwrap();
        write_proposal(
            tmp.path(),
            r#"openapi: 3.1.0
info: {title: x, version: '1'}
components:
  schemas:
    Foo:
      type: object
      properties:
        id: {type: string}
"#,
        );
        let r = validate(tmp.path()).unwrap();
        assert!(r.valid, "findings: {:?}", r.findings);
        assert_eq!(r.schemas_count, 1);
    }

    #[test]
    fn empty_proposal_is_issue() {
        let tmp = tempdir().unwrap();
        write_proposal(
            tmp.path(),
            "openapi: 3.0.3\ninfo: {title: x, version: '1'}\n",
        );
        let r = validate(tmp.path()).unwrap();
        assert!(!r.valid);
        assert!(r.findings.iter().any(|f| f.code == "empty_proposal"));
    }

    #[test]
    fn operation_without_responses_is_issue() {
        let tmp = tempdir().unwrap();
        write_proposal(
            tmp.path(),
            r#"openapi: 3.0.3
info: {title: x, version: '1'}
paths:
  /a:
    get:
      summary: missing responses
"#,
        );
        let r = validate(tmp.path()).unwrap();
        assert!(!r.valid);
        assert!(r
            .findings
            .iter()
            .any(|f| f.code == "operation_missing_responses"));
    }

    #[test]
    fn malformed_security_block_is_issue() {
        let tmp = tempdir().unwrap();
        write_proposal(
            tmp.path(),
            r#"openapi: 3.0.3
info: {title: x, version: '1'}
paths:
  /a:
    get:
      responses:
        '200': {description: ok}
      security: "permission:124012.READ"
"#,
        );
        let r = validate(tmp.path()).unwrap();
        assert!(!r.valid);
        assert!(r.findings.iter().any(|f| f.code == "security_not_list"));
    }

    #[test]
    fn openapi_2x_rejected() {
        let tmp = tempdir().unwrap();
        write_proposal(
            tmp.path(),
            r#"openapi: 2.0
info: {title: x, version: '1'}
paths:
  /a:
    get:
      responses:
        '200': {description: ok}
"#,
        );
        let r = validate(tmp.path()).unwrap();
        assert!(!r.valid);
        assert!(r.findings.iter().any(|f| f.code == "openapi_not_3x"));
    }

    #[test]
    fn warning_on_unrooted_path() {
        let tmp = tempdir().unwrap();
        write_proposal(
            tmp.path(),
            r#"openapi: 3.0.3
info: {title: x, version: '1'}
paths:
  api/no-leading-slash:
    get:
      responses:
        '200': {description: ok}
"#,
        );
        let r = validate(tmp.path()).unwrap();
        assert!(r.valid); // warning, not issue
        assert!(r.findings.iter().any(|f| f.code == "path_not_rooted"));
    }
}
