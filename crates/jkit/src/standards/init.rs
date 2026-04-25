use anyhow::{Context, Result};
use std::env;
use std::fs;

use crate::standards::list::plugin_root;

pub fn run(force: bool) -> Result<()> {
    let project_root = env::current_dir().context("getting current directory")?;
    let dest = project_root.join("docs/project-info.yaml");

    if dest.exists() && !force {
        anyhow::bail!(
            "{} already exists; pass --force to overwrite",
            dest.display()
        );
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
    println!("created {}", dest.display());
    Ok(())
}
