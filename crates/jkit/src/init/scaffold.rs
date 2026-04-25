use anyhow::{Context, Result};
use serde::Serialize;
use std::fs;
use std::path::Path;

use crate::util::{atomic_write, print_json};

const ENVRC: &str = include_str!("../../templates/scaffold/envrc");
const EXAMPLE_ENV_BODY: &str = include_str!("../../templates/scaffold/example.env");
const DOCKER_COMPOSE_YML: &str = include_str!("../../templates/scaffold/docker-compose.yml");
const OVERVIEW_MD: &str = include_str!("../../templates/scaffold/overview.md");

const GITIGNORE_FENCE_START: &str = "# --- jkit-managed (do not edit between markers) ---";
const GITIGNORE_FENCE_END: &str = "# --- end jkit-managed ---";
const GITIGNORE_ENTRIES: &[&str] = &[".env/", "target/", ".jkit/done/"];

#[derive(Serialize, Debug)]
pub struct ScaffoldReport {
    pub created: Vec<String>,
    pub existing: Vec<String>,
    pub gitignore_added: Vec<String>,
}

pub fn run() -> Result<()> {
    let cwd = std::env::current_dir().context("reading cwd")?;
    let report = scaffold(&cwd)?;
    print_json(&report)
}

pub fn scaffold(project_root: &Path) -> Result<ScaffoldReport> {
    let mut created = Vec::new();
    let mut existing = Vec::new();

    let local_env = format!(
        "# JKIT_ENV=local — fill in real values, do NOT commit.\n\n{}",
        EXAMPLE_ENV_BODY
    );
    let test_env = format!(
        "# JKIT_ENV=test — fixtures for integration tests.\n\n{}",
        EXAMPLE_ENV_BODY
    );

    let copies: &[(&str, &str)] = &[
        (".envrc", ENVRC),
        (".env/local.env", &local_env),
        (".env/test.env", &test_env),
        ("docker-compose.yml", DOCKER_COMPOSE_YML),
        ("docs/overview.md", OVERVIEW_MD),
    ];

    for (rel, contents) in copies {
        let dest = project_root.join(rel);
        if dest.exists() {
            existing.push((*rel).to_string());
        } else {
            atomic_write(&dest, contents.as_bytes())
                .with_context(|| format!("writing {}", dest.display()))?;
            created.push((*rel).to_string());
        }
    }

    let gitignore_added = merge_gitignore(project_root)?;

    Ok(ScaffoldReport {
        created,
        existing,
        gitignore_added,
    })
}

/// Idempotent merge of the canonical jkit entries into `.gitignore` under a
/// fenced block. Returns the list of entries added on this run (i.e., entries
/// that were not present in the fenced block before).
fn merge_gitignore(project_root: &Path) -> Result<Vec<String>> {
    let path = project_root.join(".gitignore");
    let original = if path.exists() {
        fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?
    } else {
        String::new()
    };

    let (existing_block, has_fence) = extract_fenced_entries(&original)?;
    let canonical: Vec<String> = GITIGNORE_ENTRIES.iter().map(|e| e.to_string()).collect();

    let added: Vec<String> = canonical
        .iter()
        .filter(|e| !existing_block.contains(*e))
        .cloned()
        .collect();

    let needs_rewrite = !has_fence || existing_block != canonical;
    if !needs_rewrite {
        return Ok(added);
    }

    let new_contents = rewrite_with_fence(&original, &canonical)?;
    atomic_write(&path, new_contents.as_bytes())
        .with_context(|| format!("writing {}", path.display()))?;

    Ok(added)
}

/// Returns (entries inside the fenced block, whether a fence was found).
/// Errors if exactly one fence marker is present (corruption).
fn extract_fenced_entries(text: &str) -> Result<(Vec<String>, bool)> {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.iter().position(|l| l.trim() == GITIGNORE_FENCE_START);
    let end = lines.iter().position(|l| l.trim() == GITIGNORE_FENCE_END);
    match (start, end) {
        (Some(s), Some(e)) if e > s => {
            let entries: Vec<String> = lines[s + 1..e]
                .iter()
                .map(|l| l.trim())
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
                .map(|l| l.to_string())
                .collect();
            Ok((entries, true))
        }
        (None, None) => Ok((Vec::new(), false)),
        _ => anyhow::bail!(
            ".gitignore contains a partial jkit fence (only one of `{}` / `{}` found); resolve manually before re-running",
            GITIGNORE_FENCE_START,
            GITIGNORE_FENCE_END
        ),
    }
}

/// Returns a new .gitignore body with the fenced jkit block rewritten to the
/// canonical entry list. If no fence existed, appends a fresh block.
fn rewrite_with_fence(original: &str, canonical: &[String]) -> Result<String> {
    let lines: Vec<&str> = original.lines().collect();
    let start = lines.iter().position(|l| l.trim() == GITIGNORE_FENCE_START);
    let end = lines.iter().position(|l| l.trim() == GITIGNORE_FENCE_END);

    let block = build_fenced_block(canonical);

    let body = match (start, end) {
        (Some(s), Some(e)) if e > s => {
            let mut out = String::new();
            for l in &lines[..s] {
                out.push_str(l);
                out.push('\n');
            }
            out.push_str(&block);
            for l in &lines[e + 1..] {
                out.push_str(l);
                out.push('\n');
            }
            out
        }
        (None, None) => {
            let mut out = String::new();
            if !original.is_empty() {
                out.push_str(original);
                if !original.ends_with('\n') {
                    out.push('\n');
                }
                out.push('\n');
            }
            out.push_str(&block);
            out
        }
        _ => unreachable!("partial-fence case rejected by extract_fenced_entries"),
    };
    Ok(body)
}

fn build_fenced_block(entries: &[String]) -> String {
    let mut s = String::new();
    s.push_str(GITIGNORE_FENCE_START);
    s.push('\n');
    for e in entries {
        s.push_str(e);
        s.push('\n');
    }
    s.push_str(GITIGNORE_FENCE_END);
    s.push('\n');
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn first_run_writes_all_files() {
        let tmp = tempdir().unwrap();
        let r = scaffold(tmp.path()).unwrap();
        assert_eq!(r.created.len(), 5);
        assert!(r.existing.is_empty());
        assert_eq!(r.gitignore_added, vec![".env/", "target/", ".jkit/done/"]);
        assert!(tmp.path().join(".envrc").is_file());
        assert!(tmp.path().join(".env/local.env").is_file());
        assert!(tmp.path().join(".env/test.env").is_file());
        assert!(tmp.path().join("docker-compose.yml").is_file());
        assert!(tmp.path().join("docs/overview.md").is_file());
        assert!(tmp.path().join(".gitignore").is_file());
    }

    #[test]
    fn idempotent_re_run() {
        let tmp = tempdir().unwrap();
        scaffold(tmp.path()).unwrap();
        let r = scaffold(tmp.path()).unwrap();
        assert!(r.created.is_empty());
        assert_eq!(r.existing.len(), 5);
        assert!(r.gitignore_added.is_empty());
    }

    #[test]
    fn local_env_header_differs_from_test_env() {
        let tmp = tempdir().unwrap();
        scaffold(tmp.path()).unwrap();
        let local = fs::read_to_string(tmp.path().join(".env/local.env")).unwrap();
        let test = fs::read_to_string(tmp.path().join(".env/test.env")).unwrap();
        assert!(local.starts_with("# JKIT_ENV=local"));
        assert!(test.starts_with("# JKIT_ENV=test"));
    }

    #[test]
    fn preserves_user_gitignore_outside_fence() {
        let tmp = tempdir().unwrap();
        let user_lines = "*.iml\nbuild/\n";
        fs::write(tmp.path().join(".gitignore"), user_lines).unwrap();
        scaffold(tmp.path()).unwrap();
        let after = fs::read_to_string(tmp.path().join(".gitignore")).unwrap();
        assert!(after.contains("*.iml"));
        assert!(after.contains("build/"));
        assert!(after.contains(".env/"));
        assert!(after.contains("target/"));
        assert!(after.contains(".jkit/done/"));
    }

    #[test]
    fn repairs_fenced_block_with_wrong_entries() {
        let tmp = tempdir().unwrap();
        let bad = format!(
            "*.iml\n\n{}\nstale-entry\n{}\n",
            GITIGNORE_FENCE_START, GITIGNORE_FENCE_END
        );
        fs::write(tmp.path().join(".gitignore"), &bad).unwrap();
        let r = scaffold(tmp.path()).unwrap();
        let after = fs::read_to_string(tmp.path().join(".gitignore")).unwrap();
        assert!(!after.contains("stale-entry"));
        assert!(after.contains(".env/"));
        assert_eq!(r.gitignore_added, vec![".env/", "target/", ".jkit/done/"]);
    }

    #[test]
    fn errors_on_partial_fence() {
        let tmp = tempdir().unwrap();
        let bad = format!("*.iml\n{}\n.env/\n", GITIGNORE_FENCE_START);
        fs::write(tmp.path().join(".gitignore"), &bad).unwrap();
        let err = scaffold(tmp.path()).unwrap_err();
        assert!(err.to_string().contains("partial jkit fence"));
    }

    #[test]
    fn second_run_with_partial_user_gitignore_only_adds_missing() {
        let tmp = tempdir().unwrap();
        let pre = format!(
            "{}\n.env/\n{}\n",
            GITIGNORE_FENCE_START, GITIGNORE_FENCE_END
        );
        fs::write(tmp.path().join(".gitignore"), &pre).unwrap();
        let r = scaffold(tmp.path()).unwrap();
        assert_eq!(r.gitignore_added, vec!["target/", ".jkit/done/"]);
    }

    #[test]
    fn skips_files_that_already_exist() {
        let tmp = tempdir().unwrap();
        fs::write(tmp.path().join(".envrc"), "# user-owned").unwrap();
        let r = scaffold(tmp.path()).unwrap();
        assert!(r.existing.contains(&".envrc".to_string()));
        assert!(r.created.contains(&".env/local.env".to_string()));
        let envrc = fs::read_to_string(tmp.path().join(".envrc")).unwrap();
        assert_eq!(envrc, "# user-owned");
    }
}
