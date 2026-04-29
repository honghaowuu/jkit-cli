use anyhow::{Context, Result};
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};

use crate::util::print_json;

#[derive(Serialize, Debug)]
pub struct Output {
    valid: bool,
    files: Vec<FileResult>,
}

#[derive(Serialize, Debug)]
pub struct FileResult {
    path: String,
    ok: bool,
    errors: Vec<String>,
}

pub fn run(files: Option<Vec<PathBuf>>) -> Result<()> {
    let cwd = std::env::current_dir().context("reading cwd")?;
    let resolved = match files {
        Some(fs_) => fs_,
        None => list_pending_paths(&cwd)?,
    };
    let output = compute(&cwd, &resolved);
    print_json(&output)
}

pub fn compute(cwd: &Path, files: &[PathBuf]) -> Output {
    let mut results = Vec::new();
    let mut all_ok = true;
    for f in files {
        let abs = if f.is_absolute() { f.clone() } else { cwd.join(f) };
        let res = validate_one(cwd, &abs);
        if !res.ok {
            all_ok = false;
        }
        results.push(res);
    }
    Output {
        valid: all_ok && !files.is_empty(),
        files: results,
    }
}

fn validate_one(cwd: &Path, path: &Path) -> FileResult {
    let display = path
        .strip_prefix(cwd)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned();
    let mut errors = Vec::new();
    let content = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            errors.push(format!("read failed: {}", e));
            return FileResult {
                path: display,
                ok: false,
                errors,
            };
        }
    };
    let (frontmatter, body) = split_frontmatter(&content);
    if body.trim().is_empty() {
        errors.push("body is empty (no non-whitespace content below frontmatter)".to_string());
    }
    if let Some(fm) = frontmatter {
        match serde_yaml::from_str::<serde_yaml::Value>(fm) {
            Ok(value) => {
                if let Some(domain) = value.get("domain").and_then(|v| v.as_str()) {
                    if !domain_in_index(cwd, domain) {
                        errors.push(format!(
                            "frontmatter domain '{}' is not declared in docs/domains.yaml — \
add it with `jkit domains add --slug {} --description <d> --use-when <w>`",
                            domain, domain
                        ));
                    }
                }
            }
            Err(e) => {
                errors.push(format!("frontmatter is not valid YAML: {}", e));
            }
        }
    }
    FileResult {
        path: display,
        ok: errors.is_empty(),
        errors,
    }
}

/// Returns (frontmatter_yaml, body). Frontmatter is the content between the first
/// `---` line and the closing `---` line. If neither boundary is found, the whole
/// input is returned as the body.
fn split_frontmatter(s: &str) -> (Option<&str>, &str) {
    let s = s.strip_prefix('\u{feff}').unwrap_or(s);
    let after = match s.strip_prefix("---\n").or_else(|| s.strip_prefix("---\r\n")) {
        Some(a) => a,
        None => return (None, s),
    };
    // Find a closing `---` on its own line.
    for sep in ["\n---\n", "\n---\r\n", "\n---"] {
        if let Some(idx) = after.find(sep) {
            let body_start = idx + sep.len();
            let body = if body_start <= after.len() {
                &after[body_start..]
            } else {
                ""
            };
            return (Some(&after[..idx]), body);
        }
    }
    (None, s)
}

/// True when the slug appears under `domains:` in `docs/domains.yaml`.
/// Returns false on missing/malformed file (we don't surface that detail
/// here — `jkit domains doctor` is the diagnostic entry point).
fn domain_in_index(cwd: &Path, slug: &str) -> bool {
    use crate::domains::yaml::DomainsFile;
    match DomainsFile::load(cwd) {
        Ok(f) => f.get(slug).is_some(),
        Err(_) => false,
    }
}

fn list_pending_paths(cwd: &Path) -> Result<Vec<PathBuf>> {
    let dir = cwd.join("docs/changes/pending");
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut paths: Vec<PathBuf> = fs::read_dir(&dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("md"))
        .collect();
    paths.sort();
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn write(p: &Path, contents: &str) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, contents).unwrap();
    }

    #[test]
    fn valid_file_with_no_frontmatter_passes() {
        let tmp = tempdir().unwrap();
        write(
            &tmp.path().join("docs/changes/pending/x.md"),
            "Just a description.\n",
        );
        let out = compute(
            tmp.path(),
            &[PathBuf::from("docs/changes/pending/x.md")],
        );
        assert!(out.valid, "{:?}", out);
    }

    fn write_domains_yaml(tmp: &Path, slugs: &[&str]) {
        let mut yaml = String::from("domains:\n");
        for s in slugs {
            yaml.push_str(&format!(
                "  {s}:\n    description: x\n    use_when: x\n"
            ));
        }
        write(&tmp.join("docs/domains.yaml"), &yaml);
    }

    #[test]
    fn empty_body_is_rejected() {
        let tmp = tempdir().unwrap();
        write(
            &tmp.path().join("docs/changes/pending/x.md"),
            "---\ndomain: billing\n---\n\n   \n",
        );
        write_domains_yaml(tmp.path(), &["billing"]);
        let out = compute(
            tmp.path(),
            &[PathBuf::from("docs/changes/pending/x.md")],
        );
        assert!(!out.valid);
        assert!(out.files[0].errors.iter().any(|e| e.contains("body is empty")));
    }

    #[test]
    fn frontmatter_slug_not_in_domains_yaml_is_rejected() {
        let tmp = tempdir().unwrap();
        write(
            &tmp.path().join("docs/changes/pending/x.md"),
            "---\ndomain: nonsense\n---\n\nA real body paragraph.\n",
        );
        write_domains_yaml(tmp.path(), &["billing"]);
        let out = compute(
            tmp.path(),
            &[PathBuf::from("docs/changes/pending/x.md")],
        );
        assert!(!out.valid);
        assert!(out.files[0]
            .errors
            .iter()
            .any(|e| e.contains("'nonsense'") && e.contains("docs/domains.yaml")));
    }

    #[test]
    fn frontmatter_slug_in_domains_yaml_passes() {
        let tmp = tempdir().unwrap();
        write(
            &tmp.path().join("docs/changes/pending/x.md"),
            "---\ndomain: billing\n---\n\nA real body paragraph.\n",
        );
        write_domains_yaml(tmp.path(), &["billing"]);
        let out = compute(
            tmp.path(),
            &[PathBuf::from("docs/changes/pending/x.md")],
        );
        assert!(out.valid, "{:?}", out);
    }

    #[test]
    fn missing_domains_yaml_rejects_any_frontmatter_slug() {
        let tmp = tempdir().unwrap();
        write(
            &tmp.path().join("docs/changes/pending/x.md"),
            "---\ndomain: billing\n---\n\nA real body paragraph.\n",
        );
        // No docs/domains.yaml at all.
        let out = compute(
            tmp.path(),
            &[PathBuf::from("docs/changes/pending/x.md")],
        );
        assert!(!out.valid);
        assert!(out.files[0]
            .errors
            .iter()
            .any(|e| e.contains("docs/domains.yaml")));
    }

    #[test]
    fn no_files_means_not_ok() {
        let tmp = tempdir().unwrap();
        let out = compute(tmp.path(), &[]);
        assert!(!out.valid);
        assert!(out.files.is_empty());
    }

    #[test]
    fn read_failure_surfaces_as_error() {
        let tmp = tempdir().unwrap();
        let out = compute(
            tmp.path(),
            &[PathBuf::from("docs/changes/pending/missing.md")],
        );
        assert!(!out.valid);
        assert!(out.files[0].errors[0].contains("read failed"));
    }

    #[test]
    fn split_frontmatter_handles_crlf() {
        let s = "---\r\ndomain: billing\r\n---\r\nbody here\r\n";
        let (fm, body) = split_frontmatter(s);
        assert!(fm.is_some());
        assert!(body.contains("body here"));
    }

    #[test]
    fn split_frontmatter_no_close_returns_no_frontmatter() {
        let s = "---\nincomplete\n";
        let (fm, body) = split_frontmatter(s);
        assert!(fm.is_none());
        assert_eq!(body, s);
    }
}
