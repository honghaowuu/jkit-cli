use anyhow::{Context, Result};
use chrono::Utc;
use regex::Regex;
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::util::print_json;

#[derive(Serialize)]
struct PlaceReport {
    source: String,
    destination: String,
    next_index: String,
    git_staged: bool,
}

pub fn run(run_dir: &Path, feature: &str, flyway_dir: &Path) -> Result<()> {
    let migration_dir = run_dir.join("migration");
    if !migration_dir.exists() {
        anyhow::bail!("no migration SQL staged ({} missing)", migration_dir.display());
    }
    let mut sql_files: Vec<PathBuf> = Vec::new();
    for entry in fs::read_dir(&migration_dir)
        .with_context(|| format!("reading {}", migration_dir.display()))?
    {
        let entry = entry?;
        let p = entry.path();
        if p.is_file() {
            if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                if name.starts_with('V') && name.ends_with(".sql") {
                    sql_files.push(p);
                }
            }
        }
    }
    match sql_files.len() {
        0 => anyhow::bail!("no migration SQL staged"),
        1 => {}
        n => anyhow::bail!("more than one migration SQL in run; expected exactly one ({} found)", n),
    }
    let source = sql_files.remove(0);

    if !flyway_dir.exists() {
        fs::create_dir_all(flyway_dir).with_context(|| {
            format!("creating flyway dir {}", flyway_dir.display())
        })?;
    }
    let next_index = next_index(flyway_dir)?;
    let date = Utc::now().format("%Y%m%d").to_string();
    let dest_name = format!("V{}_{}__{}.sql", date, next_index, feature);
    let dest = flyway_dir.join(&dest_name);
    if dest.exists() {
        anyhow::bail!("destination collision at {}", dest.display());
    }

    fs::rename(&source, &dest)
        .with_context(|| format!("renaming {} -> {}", source.display(), dest.display()))?;

    let git_staged = git_add(&dest);

    let report = PlaceReport {
        source: source.to_string_lossy().into_owned(),
        destination: dest.to_string_lossy().into_owned(),
        next_index,
        git_staged,
    };
    print_json(&report)
}

fn next_index(flyway_dir: &Path) -> Result<String> {
    let re = Regex::new(r"^V[^_]+_([0-9]+)__.*\.sql$").unwrap();
    let mut max_idx: u32 = 0;
    let mut found_any = false;
    for entry in fs::read_dir(flyway_dir)? {
        let entry = entry?;
        if let Some(name) = entry.file_name().to_str() {
            if let Some(cap) = re.captures(name) {
                if let Ok(n) = cap[1].parse::<u32>() {
                    found_any = true;
                    if n > max_idx {
                        max_idx = n;
                    }
                }
            }
        }
    }
    let next = if found_any { max_idx + 1 } else { 1 };
    Ok(format!("{:03}", next))
}

fn git_add(path: &Path) -> bool {
    let out = Command::new("git").arg("add").arg(path).output();
    match out {
        Ok(o) if o.status.success() => true,
        Ok(_) => {
            eprintln!("warning: git add failed");
            false
        }
        Err(e) => {
            eprintln!("warning: git add: {}", e);
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn next_index_empty_dir_is_001() {
        let tmp = tempdir().unwrap();
        assert_eq!(next_index(tmp.path()).unwrap(), "001");
    }

    #[test]
    fn next_index_picks_max_plus_one() {
        let tmp = tempdir().unwrap();
        fs::write(tmp.path().join("V20260101_001__a.sql"), "").unwrap();
        fs::write(tmp.path().join("V20260102_007__b.sql"), "").unwrap();
        fs::write(tmp.path().join("V20260103_005__c.sql"), "").unwrap();
        assert_eq!(next_index(tmp.path()).unwrap(), "008");
    }

    #[test]
    fn next_index_ignores_non_matching() {
        let tmp = tempdir().unwrap();
        fs::write(tmp.path().join("README.md"), "").unwrap();
        fs::write(tmp.path().join("V_no_index.sql"), "").unwrap();
        assert_eq!(next_index(tmp.path()).unwrap(), "001");
    }
}
