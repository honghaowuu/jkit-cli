use anyhow::{Context, Result};
use serde::Serialize;
use std::fs;
use std::path::Path;

use crate::util::print_json;

#[derive(Serialize, Debug)]
pub struct BootstrapReport {
    pub created: Vec<String>,
    pub existing: Vec<String>,
}

pub fn run() -> Result<()> {
    let cwd = std::env::current_dir().context("reading cwd")?;
    let report = bootstrap(&cwd)?;
    print_json(&report)
}

pub fn bootstrap(cwd: &Path) -> Result<BootstrapReport> {
    let mut created = Vec::new();
    let mut existing = Vec::new();

    for sub in ["docs/changes/pending", "docs/changes/done"] {
        let dir = cwd.join(sub);
        if dir.is_dir() {
            existing.push(sub.to_string());
        } else {
            fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
            created.push(sub.to_string());
        }
        // Always ensure .gitkeep so empty dirs are tracked.
        let gitkeep = dir.join(".gitkeep");
        if !gitkeep.exists() {
            fs::write(&gitkeep, b"").with_context(|| format!("writing {}", gitkeep.display()))?;
        }
    }

    Ok(BootstrapReport { created, existing })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn first_run_creates_both() {
        let tmp = tempdir().unwrap();
        let r = bootstrap(tmp.path()).unwrap();
        assert_eq!(r.created.len(), 2);
        assert!(r.existing.is_empty());
        assert!(tmp.path().join("docs/changes/pending/.gitkeep").is_file());
        assert!(tmp.path().join("docs/changes/done/.gitkeep").is_file());
    }

    #[test]
    fn idempotent_on_existing_dirs() {
        let tmp = tempdir().unwrap();
        bootstrap(tmp.path()).unwrap();
        let second = bootstrap(tmp.path()).unwrap();
        assert!(second.created.is_empty());
        assert_eq!(second.existing.len(), 2);
    }

    #[test]
    fn existing_dir_without_gitkeep_gets_one() {
        let tmp = tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("docs/changes/pending")).unwrap();
        let r = bootstrap(tmp.path()).unwrap();
        assert!(r.existing.contains(&"docs/changes/pending".to_string()));
        assert!(tmp.path().join("docs/changes/pending/.gitkeep").is_file());
    }
}
