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

/// Print a value as compact JSON to stdout with a trailing newline.
pub fn print_json<T: serde::Serialize>(value: &T) -> Result<()> {
    let s = serde_json::to_string(value).context("serializing JSON")?;
    println!("{}", s);
    Ok(())
}
