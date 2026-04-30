use std::path::PathBuf;
use std::process::Command;
use tempfile::tempdir;

fn jkit_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_jkit"))
}

fn plugin_root() -> PathBuf {
    // Workspace layout: jkit-cli/crates/jkit/ → ../../../jkit (sibling repo).
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../jkit")
        .canonicalize()
        .expect("plugin repo not at ../../../jkit; rerun integration tests from a workspace that has it")
}

#[test]
fn init_umbrella_creates_full_layout() {
    let tmp = tempdir().unwrap();
    let out = Command::new(jkit_bin())
        .arg("init")
        .current_dir(tmp.path())
        .env("JKIT_PLUGIN_ROOT", plugin_root())
        .output()
        .expect("running jkit init");
    assert!(
        out.status.success(),
        "jkit init failed: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8(out.stdout).unwrap();
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("stdout is JSON");
    assert_eq!(json["ok"], true);

    let si = &json["steps"]["standards_init"];
    assert_eq!(si["created"], serde_json::json!(["docs/project-info.yaml"]));

    let sc = &json["steps"]["scaffold"];
    assert_eq!(sc["created"].as_array().unwrap().len(), 5);
    assert_eq!(
        sc["gitignore_added"],
        serde_json::json!([".env/", "target/"])
    );

    let next: Vec<&str> = json["next_steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(next.iter().any(|s| s.contains("project-info.yaml")));
    assert!(next.iter().any(|s| s.contains("overview.md")));
    assert!(next.iter().any(|s| s.contains("/start-feature")));

    for rel in [
        "docs/project-info.yaml",
        ".envrc",
        ".env/local.env",
        ".env/test.env",
        "docker-compose.yml",
        "docs/overview.md",
        ".gitignore",
    ] {
        assert!(
            tmp.path().join(rel).is_file(),
            "expected {} to exist",
            rel
        );
    }
    // The pending/done inboxes are gone — branches replace them.
    assert!(!tmp.path().join("docs/changes").exists());
}

#[test]
fn init_umbrella_idempotent() {
    let tmp = tempdir().unwrap();
    Command::new(jkit_bin())
        .arg("init")
        .current_dir(tmp.path())
        .env("JKIT_PLUGIN_ROOT", plugin_root())
        .output()
        .unwrap();

    let out = Command::new(jkit_bin())
        .arg("init")
        .current_dir(tmp.path())
        .env("JKIT_PLUGIN_ROOT", plugin_root())
        .output()
        .unwrap();
    assert!(out.status.success());

    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(json["steps"]["standards_init"]["created"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(json["steps"]["scaffold"]["created"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(json["steps"]["scaffold"]["gitignore_added"]
        .as_array()
        .unwrap()
        .is_empty());
}

#[test]
fn init_scaffold_subcommand_only_runs_scaffold_step() {
    let tmp = tempdir().unwrap();
    let out = Command::new(jkit_bin())
        .args(["init", "scaffold"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(out.status.success());

    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["created"].as_array().unwrap().len(), 5);
    // Did NOT run standards::init.
    assert!(!tmp.path().join("docs/project-info.yaml").exists());
}
