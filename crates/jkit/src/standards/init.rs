use anyhow::{Context, Result};
use serde::Serialize;
use std::env;
use std::fs;
use std::path::Path;

use crate::standards::list::plugin_root;
use crate::util::print_json;

#[derive(Serialize, Debug)]
pub struct StandardsInitReport {
    pub created: Vec<String>,
    pub existing: Vec<String>,
}

/// CLI entry point. Preserves the historical contract: bail with a non-zero
/// exit when `docs/project-info.yaml` already exists and `--force` is not set.
pub fn run(force: bool) -> Result<()> {
    let project_root = env::current_dir().context("getting current directory")?;
    let dest_rel = "docs/project-info.yaml";
    let dest = project_root.join(dest_rel);
    if dest.exists() && !force {
        anyhow::bail!(
            "{} already exists; pass --force to overwrite",
            dest.display()
        );
    }
    let report = init(&project_root, force)?;
    print_json(&report)
}

/// Library entry point used by `jkit init` umbrella. Skips if the file exists
/// (without --force), copies otherwise. Never bails on existing-file.
pub fn init(project_root: &Path, force: bool) -> Result<StandardsInitReport> {
    let dest_rel = "docs/project-info.yaml";
    let dest = project_root.join(dest_rel);

    if dest.exists() && !force {
        return Ok(StandardsInitReport {
            created: vec![],
            existing: vec![dest_rel.to_string()],
        });
    }

    let plugin_root = plugin_root()?;
    let template = plugin_root.join("docs/project-info.schema.yaml");
    if !template.exists() {
        anyhow::bail!("shipped template missing: {}", template.display());
    }

    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    fs::copy(&template, &dest)
        .with_context(|| format!("copying {} -> {}", template.display(), dest.display()))?;
    Ok(StandardsInitReport {
        created: vec![dest_rel.to_string()],
        existing: vec![],
    })
}
