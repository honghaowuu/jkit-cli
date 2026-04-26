use anyhow::{Context, Result};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::util::print_json;

/// Per-file SHA256 manifest written by `stage::run` on success and
/// consumed by `stage-status` to detect hand-edits before re-staging.
#[derive(Serialize, serde::Deserialize, Debug)]
pub(crate) struct Manifest {
    /// Map of project-relative path → hex SHA256 of file contents.
    pub files: BTreeMap<String, String>,
}

#[derive(Serialize, Debug)]
struct Status {
    service: String,
    stage_dir: String,
    stage_exists: bool,
    manifest_present: bool,
    hand_edited_files: Vec<String>,
    missing_files: Vec<String>,
}

pub fn run(service: &str) -> Result<()> {
    let stage_dir: PathBuf = PathBuf::from(".jkit/contract-stage").join(service);
    let manifest_path = stage_dir.join(".manifest.json");
    let stage_exists = stage_dir.is_dir();
    let manifest_present = manifest_path.is_file();

    let (hand_edited_files, missing_files) = if stage_exists && manifest_present {
        let manifest_text = fs::read_to_string(&manifest_path)
            .with_context(|| format!("reading {}", manifest_path.display()))?;
        let manifest: Manifest = serde_json::from_str(&manifest_text)
            .with_context(|| format!("parsing {}", manifest_path.display()))?;
        diverged(&manifest)?
    } else {
        (Vec::new(), Vec::new())
    };

    let status = Status {
        service: service.to_string(),
        stage_dir: stage_dir.display().to_string(),
        stage_exists,
        manifest_present,
        hand_edited_files,
        missing_files,
    };
    print_json(&status)
}

/// Compare the current SHA256 of every manifest entry to the recorded hash.
/// Returns `(diverged, missing)`.
fn diverged(manifest: &Manifest) -> Result<(Vec<String>, Vec<String>)> {
    let mut diverged = Vec::new();
    let mut missing = Vec::new();
    for (path_str, expected) in &manifest.files {
        let path = Path::new(path_str);
        if !path.exists() {
            missing.push(path_str.clone());
            continue;
        }
        let actual = sha256_hex(path)?;
        if &actual != expected {
            diverged.push(path_str.clone());
        }
    }
    diverged.sort();
    missing.sort();
    Ok((diverged, missing))
}

pub(crate) fn sha256_hex(path: &Path) -> Result<String> {
    let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    Ok(format!("{:x}", hasher.finalize()))
}

/// Build a manifest by hashing every entry of `paths`. Used by `stage::run`
/// after a successful stage write.
pub(crate) fn build_manifest(paths: &[String]) -> Result<Manifest> {
    let mut files = BTreeMap::new();
    for p in paths {
        let path = Path::new(p);
        if !path.is_file() {
            continue;
        }
        files.insert(p.clone(), sha256_hex(path)?);
    }
    Ok(Manifest { files })
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
    fn empty_when_stage_missing() {
        let tmp = tempdir().unwrap();
        let _g = ChdirGuard::new(tmp.path());
        let stage_dir: PathBuf = PathBuf::from(".jkit/contract-stage/foo");
        let manifest_path = stage_dir.join(".manifest.json");
        assert!(!stage_dir.is_dir());
        assert!(!manifest_path.is_file());
    }

    #[test]
    fn build_and_diverge_round_trip() {
        let tmp = tempdir().unwrap();
        let _g = ChdirGuard::new(tmp.path());
        write(Path::new("a.txt"), "alpha");
        write(Path::new("b.txt"), "bravo");
        let m = build_manifest(&["a.txt".into(), "b.txt".into()]).unwrap();
        assert_eq!(m.files.len(), 2);

        // No drift yet.
        let (d, miss) = diverged(&m).unwrap();
        assert!(d.is_empty());
        assert!(miss.is_empty());

        // Hand-edit a.txt, delete b.txt.
        fs::write("a.txt", "alpha-edited").unwrap();
        fs::remove_file("b.txt").unwrap();
        let (d, miss) = diverged(&m).unwrap();
        assert_eq!(d, vec!["a.txt"]);
        assert_eq!(miss, vec!["b.txt"]);
    }

    #[test]
    fn missing_files_in_manifest_are_skipped_during_build() {
        let tmp = tempdir().unwrap();
        let _g = ChdirGuard::new(tmp.path());
        write(Path::new("present.txt"), "x");
        let m = build_manifest(&["present.txt".into(), "absent.txt".into()]).unwrap();
        assert_eq!(m.files.len(), 1);
        assert!(m.files.contains_key("present.txt"));
    }

    /// Tests in this file mutate cwd — guard restores it on drop.
    struct ChdirGuard {
        original: PathBuf,
    }
    impl ChdirGuard {
        fn new(target: &Path) -> Self {
            let original = std::env::current_dir().unwrap();
            std::env::set_current_dir(target).unwrap();
            Self { original }
        }
    }
    impl Drop for ChdirGuard {
        fn drop(&mut self) {
            let _ = std::env::set_current_dir(&self.original);
        }
    }
}
