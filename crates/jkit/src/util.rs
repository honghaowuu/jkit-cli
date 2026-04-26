use anyhow::{Context, Result};
use std::fs;
use std::io::Write;
use std::path::Path;

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
