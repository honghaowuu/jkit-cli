use std::process::Command;
use tempfile::tempdir;

const POM_BASIC: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<project xmlns="http://maven.apache.org/POM/4.0.0">
    <modelVersion>4.0.0</modelVersion>
    <groupId>com.example</groupId>
    <artifactId>demo</artifactId>
    <version>1.0.0</version>
    <dependencies>
    </dependencies>
</project>
"#;

#[test]
fn dry_run_reports_missing_jacoco() {
    let dir = tempdir().unwrap();
    let pom = dir.path().join("pom.xml");
    std::fs::write(&pom, POM_BASIC).unwrap();

    let bin = env!("CARGO_BIN_EXE_jkit");
    let out = Command::new(bin)
        .args(["pom", "prereqs", "--profile", "jacoco", "--pom"])
        .arg(&pom)
        .output()
        .expect("run pom prereqs");
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid json");
    assert_eq!(v["profile"], "jacoco");
    assert_eq!(v["ready"], false);
    assert_eq!(v["missing"][0], "jacoco-maven-plugin");
    let unchanged = std::fs::read_to_string(&pom).unwrap();
    assert_eq!(unchanged, POM_BASIC, "dry-run must not modify pom");
}

#[test]
fn apply_inserts_jacoco_plugin() {
    let dir = tempdir().unwrap();
    let pom = dir.path().join("pom.xml");
    std::fs::write(&pom, POM_BASIC).unwrap();

    let bin = env!("CARGO_BIN_EXE_jkit");
    let out = Command::new(bin)
        .args(["pom", "prereqs", "--profile", "jacoco", "--apply", "--pom"])
        .arg(&pom)
        .output()
        .expect("run pom prereqs --apply");
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let modified = std::fs::read_to_string(&pom).unwrap();
    assert!(modified.contains("jacoco-maven-plugin"), "expected plugin inserted");
    assert!(modified.contains("<build>"), "expected <build> created");

    // Re-running should be a no-op.
    let out2 = Command::new(bin)
        .args(["pom", "prereqs", "--profile", "jacoco", "--apply", "--pom"])
        .arg(&pom)
        .output()
        .expect("run pom prereqs --apply (second time)");
    let v2: serde_json::Value = serde_json::from_slice(&out2.stdout).expect("valid json");
    assert_eq!(v2["already_present"][0], "jacoco-maven-plugin");
    assert!(v2["actions_taken"].as_array().unwrap().is_empty());
}

#[test]
fn add_dep_inserts_with_scope() {
    let dir = tempdir().unwrap();
    let pom = dir.path().join("pom.xml");
    std::fs::write(&pom, POM_BASIC).unwrap();

    let bin = env!("CARGO_BIN_EXE_jkit");
    let out = Command::new(bin)
        .args([
            "pom", "add-dep", "--group-id", "io.example", "--artifact-id", "billing-api",
            "--version", "1.2.0", "--scope", "compile", "--apply", "--pom",
        ])
        .arg(&pom)
        .output()
        .expect("run pom add-dep");
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let modified = std::fs::read_to_string(&pom).unwrap();
    assert!(modified.contains("<artifactId>billing-api</artifactId>"));
    assert!(modified.contains("<scope>compile</scope>"));
}

#[test]
fn add_dep_version_mismatch_does_not_modify() {
    let pom_with = r#"<?xml version="1.0"?>
<project>
    <modelVersion>4.0.0</modelVersion>
    <groupId>x</groupId>
    <artifactId>y</artifactId>
    <version>1</version>
    <dependencies>
        <dependency>
            <groupId>io.example</groupId>
            <artifactId>billing-api</artifactId>
            <version>0.9.0</version>
        </dependency>
    </dependencies>
</project>
"#;
    let dir = tempdir().unwrap();
    let pom = dir.path().join("pom.xml");
    std::fs::write(&pom, pom_with).unwrap();

    let bin = env!("CARGO_BIN_EXE_jkit");
    let out = Command::new(bin)
        .args([
            "pom", "add-dep", "--group-id", "io.example", "--artifact-id", "billing-api",
            "--version", "1.2.0", "--apply", "--pom",
        ])
        .arg(&pom)
        .output()
        .expect("run pom add-dep");
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["fragments"][0]["action"], "version_mismatch");
    let unchanged = std::fs::read_to_string(&pom).unwrap();
    assert!(unchanged.contains("<version>0.9.0</version>"));
    assert!(!unchanged.contains("<version>1.2.0</version>"));
}
