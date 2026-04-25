use anyhow::{Context, Result};
use std::env;
use std::path::PathBuf;

use crate::standards::config::ProjectInfo;
use crate::standards::gates::evaluate;

pub fn run(explain: bool) -> Result<()> {
    let project_root = env::current_dir().context("getting current directory")?;
    let project_info_path = project_root.join("docs/project-info.yaml");
    if !project_info_path.exists() {
        anyhow::bail!(
            "missing {}\n  run `jkit standards init` to create it from the shipped template",
            project_info_path.display()
        );
    }
    let info = ProjectInfo::from_yaml_file(&project_info_path)?;
    let plugin_root = plugin_root()?;
    let outcomes = evaluate(&info);

    if explain {
        for o in &outcomes {
            let verb = if o.applies { "applies" } else { "skipped" };
            println!("{:<18} {} ({})", o.file.short_name(), verb, o.reason);
        }
    } else {
        for o in &outcomes {
            if o.applies {
                let path = plugin_root.join(o.file.relative_path());
                println!("{}", path.display());
            }
        }
    }
    Ok(())
}

pub(crate) fn plugin_root() -> Result<PathBuf> {
    if let Ok(v) = env::var("JKIT_PLUGIN_ROOT") {
        return Ok(PathBuf::from(v));
    }
    let exe = env::current_exe().context("locating jkit executable")?;
    let bin_dir = exe.parent().context("exe has no parent")?;
    let plugin_root = bin_dir.parent().context("bin dir has no parent")?;
    Ok(plugin_root.to_path_buf())
}
