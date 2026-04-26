use anyhow::{Context, Result};
use fs2::FileExt;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

/// RAII exclusive file lock for serializing read-modify-write operations on
/// shared on-disk state (e.g. Flyway migration index allocation). The lock is
/// released when the guard is dropped.
pub struct LockGuard {
    file: fs::File,
    #[allow(dead_code)]
    path: PathBuf,
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

/// Acquire an exclusive `flock(2)` on `<dir>/<name>.lock`, creating the file
/// if needed. Blocks until the lock is available.
pub fn lock_file_in(dir: &Path, name: &str) -> Result<LockGuard> {
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = dir.join(format!("{name}.lock"));
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .with_context(|| format!("opening lock file {}", path.display()))?;
    file.lock_exclusive()
        .with_context(|| format!("acquiring lock on {}", path.display()))?;
    Ok(LockGuard { file, path })
}

/// Atomic-ish file write: write to a sibling tempfile then rename.
pub fn atomic_write(path: &Path, contents: &[u8]) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    if !parent.as_os_str().is_empty() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating parent dir {}", parent.display()))?;
    }
    let tmp = parent.join(format!(
        ".jkit-tmp-{}-{}",
        std::process::id(),
        path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "out".to_string())
    ));
    {
        let mut f = fs::File::create(&tmp)
            .with_context(|| format!("creating temp file {}", tmp.display()))?;
        f.write_all(contents)
            .with_context(|| format!("writing temp file {}", tmp.display()))?;
        f.sync_all().ok();
    }
    fs::rename(&tmp, path)
        .with_context(|| format!("renaming {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}

/// Print `value` as a Phase A success envelope (`{"ok": true, ...fields}`) to
/// stdout, then exit 0. Diverges; the declared `Result<()>` return type lets
/// existing tail-position call sites (`return print_json(&x);`, `print_json(&x)?;`)
/// compile unchanged.
///
/// `value` MUST serialize to a JSON object so its fields can be flattened
/// alongside the envelope marker. Top-level arrays should be wrapped at the
/// call site (e.g. `print_json(&json!({"items": vec}))`) before being passed in.
pub fn print_json<T: serde::Serialize>(value: &T) -> Result<()> {
    let v = serde_json::to_value(value).context("serializing JSON")?;
    crate::envelope::print_ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn lock_file_in_serializes_concurrent_acquires() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();

        let first = lock_file_in(&path, "test").expect("first lock");

        let (tx, rx) = mpsc::channel();
        let path_clone = path.clone();
        let handle = thread::spawn(move || {
            let _second = lock_file_in(&path_clone, "test").expect("second lock");
            tx.send(()).ok();
        });

        assert!(
            rx.recv_timeout(Duration::from_millis(150)).is_err(),
            "second lock acquired while first was still held"
        );

        drop(first);
        rx.recv_timeout(Duration::from_secs(2))
            .expect("second lock should acquire after first dropped");
        handle.join().unwrap();
    }
}
