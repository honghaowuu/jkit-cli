//! `jkit permissions list` — aggregate every `@Permission` code currently
//! in use across `src/main/java`. Used when a new run needs to allocate a
//! permission code: the human (or skill) sees the existing set and picks
//! something non-colliding.
//!
//! Implementation is a regex sweep — `@Permission` codes are short string
//! literals (`"124012.MANAGE"` etc.) referenced from `PowerConst` constants
//! per the auth-toms standard, but consumers also hardcode them in tests
//! and migration shims; both surface here.

use anyhow::{Context, Result};
use clap::Subcommand;
use regex::Regex;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

use crate::util::print_json;

#[derive(Subcommand, Debug)]
pub enum PermissionsCmd {
    /// Print every distinct permission code referenced under `--src` (and
    /// where each one appears) so a new run can allocate a non-colliding
    /// code. Read-only.
    List {
        #[arg(long, default_value = "src/main/java")]
        src: PathBuf,
    },
}

pub fn run(cmd: PermissionsCmd) -> Result<()> {
    match cmd {
        PermissionsCmd::List { src } => list(&src),
    }
}

#[derive(Serialize)]
struct ListReport {
    src: String,
    count: usize,
    codes: Vec<CodeEntry>,
}

#[derive(Serialize)]
struct CodeEntry {
    code: String,
    /// Source files that reference this code, sorted unique.
    files: Vec<String>,
}

pub fn list(src: &Path) -> Result<()> {
    let cwd = std::env::current_dir().context("reading cwd")?;
    let src_abs = if src.is_absolute() {
        src.to_path_buf()
    } else {
        cwd.join(src)
    };
    if !src_abs.exists() {
        anyhow::bail!("src dir not found: {}", src.display());
    }

    // <module>.<OPERATION> — module is digits (jkit projects use 6-digit
    // module codes per auth-toms.md PowerConst examples); operation is
    // SCREAMING_SNAKE_CASE. Match within a string literal so we don't pick
    // up non-permission constants like ipv4 octets.
    let re = Regex::new(r#""([0-9]{4,8}\.[A-Z][A-Z0-9_]*)""#).unwrap();

    let mut by_code: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for entry in WalkDir::new(&src_abs).into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("java") {
            continue;
        }
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(_) => continue,
        };
        let rel = path
            .strip_prefix(&cwd)
            .unwrap_or(path)
            .to_string_lossy()
            .into_owned();
        let mut local: std::collections::BTreeSet<String> = Default::default();
        for cap in re.captures_iter(&text) {
            local.insert(cap[1].to_string());
        }
        for code in local {
            let files = by_code.entry(code).or_default();
            if !files.contains(&rel) {
                files.push(rel.clone());
            }
        }
    }

    for files in by_code.values_mut() {
        files.sort();
    }

    let codes: Vec<CodeEntry> = by_code
        .into_iter()
        .map(|(code, files)| CodeEntry { code, files })
        .collect();

    let report = ListReport {
        src: src_abs
            .strip_prefix(&cwd)
            .unwrap_or(&src_abs)
            .to_string_lossy()
            .into_owned(),
        count: codes.len(),
        codes,
    };
    print_json(&report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn write(p: &Path, s: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, s).unwrap();
    }

    #[test]
    fn extraction_picks_module_dot_operation_only() {
        let re = Regex::new(r#""([0-9]{4,8}\.[A-Z][A-Z0-9_]*)""#).unwrap();
        let s = r#"
            String good = "124012.MANAGE";
            String alsoGood = "9999.OK";
            String tooShort = "12.OK";
            String version = "1.2.3";
            String ip = "192.168.1.1";
            String label = "moduleX.MANAGE";
        "#;
        let hits: Vec<&str> = re.captures_iter(s).map(|c| c.get(1).unwrap().as_str()).collect();
        assert_eq!(hits, vec!["124012.MANAGE", "9999.OK"]);
    }
}
