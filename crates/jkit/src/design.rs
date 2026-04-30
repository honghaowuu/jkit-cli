//! Shared reader for `<run>/design.md` frontmatter.
//!
//! `/start-feature` writes `<run>/design.md` with YAML frontmatter:
//! ```text
//! ---
//! feature: <slug>
//! domains: [<slug>, <slug>]
//! ---
//! ```
//! Downstream consumers (`jkit scenarios gap --run`, `jkit coverage
//! --scope run:`, `jkit migration diff`) read the same file to learn which
//! domains the run touches.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::fs;
use std::path::Path;

#[derive(Debug, Deserialize, Default)]
pub struct DesignFrontmatter {
    #[serde(default)]
    pub feature: Option<String>,
    #[serde(default)]
    pub domains: Vec<String>,
}

/// Read `<run_dir>/design.md` and parse its YAML frontmatter. Errors if the
/// file is missing, frontmatter is absent / unterminated, or YAML is invalid.
pub fn read_design(run_dir: &Path) -> Result<DesignFrontmatter> {
    let path = run_dir.join("design.md");
    if !path.exists() {
        anyhow::bail!("design.md missing in {}", run_dir.display());
    }
    let text =
        fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    parse(&text).with_context(|| format!("parsing frontmatter in {}", path.display()))
}

/// Parse the YAML frontmatter from a design.md document body.
pub fn parse(text: &str) -> Result<DesignFrontmatter> {
    let trimmed = text.trim_start_matches('\u{feff}');
    let mut lines = trimmed.lines();
    let first = lines
        .next()
        .ok_or_else(|| anyhow::anyhow!("design.md is empty"))?;
    if first.trim() != "---" {
        anyhow::bail!("design.md does not start with `---`");
    }
    let mut yaml = String::new();
    let mut found_end = false;
    for line in lines {
        if line.trim() == "---" {
            found_end = true;
            break;
        }
        yaml.push_str(line);
        yaml.push('\n');
    }
    if !found_end {
        anyhow::bail!("design.md frontmatter is not terminated by a `---` line");
    }
    let fm: DesignFrontmatter =
        serde_yaml::from_str(&yaml).context("YAML frontmatter is invalid")?;
    Ok(fm)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn parses_inline_array_form() {
        let md = "---\nfeature: bulk-invoice\ndomains: [billing, inventory]\n---\n\n# Title\n";
        let fm = parse(md).unwrap();
        assert_eq!(fm.feature.as_deref(), Some("bulk-invoice"));
        assert_eq!(fm.domains, vec!["billing", "inventory"]);
    }

    #[test]
    fn parses_block_array_form() {
        let md = "---\nfeature: x\ndomains:\n  - billing\n  - inventory\n---\nbody";
        let fm = parse(md).unwrap();
        assert_eq!(fm.domains, vec!["billing", "inventory"]);
    }

    #[test]
    fn missing_opening_fence_errors() {
        let err = parse("# No frontmatter\n\nbody").unwrap_err();
        assert!(err.to_string().contains("`---`"));
    }

    #[test]
    fn unterminated_frontmatter_errors() {
        let err = parse("---\nfeature: x\nstill in frontmatter\n").unwrap_err();
        assert!(err.to_string().contains("not terminated"));
    }

    #[test]
    fn does_not_read_body_separator_as_close() {
        let md = "---\nfeature: x\ndomains: [a]\n---\n\n## Section\n\n---\n\nmore body\n";
        let fm = parse(md).unwrap();
        assert_eq!(fm.domains, vec!["a"]);
    }

    #[test]
    fn read_design_missing_file_errors() {
        let tmp = tempdir().unwrap();
        let err = read_design(tmp.path()).unwrap_err();
        assert!(err.to_string().contains("design.md missing"));
    }

    #[test]
    fn read_design_reads_real_file() {
        let tmp = tempdir().unwrap();
        std::fs::write(
            tmp.path().join("design.md"),
            "---\nfeature: x\ndomains: [billing]\n---\nbody\n",
        )
        .unwrap();
        let fm = read_design(tmp.path()).unwrap();
        assert_eq!(fm.domains, vec!["billing"]);
    }
}
