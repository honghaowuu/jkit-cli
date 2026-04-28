use anyhow::Result;
use serde::Serialize;
use std::fs;
use std::path::Path;

use super::build_tool::{detect, BuildTool, DetectedBuild};
use crate::pom::xml::{child_exists, child_text, dependencies_span, plugins_span, read_pom};
use crate::util::print_json;

const FLYWAY_GROUP: &str = "org.flywaydb";
const FLYWAY_LIB_ARTIFACT: &str = "flyway-core";
const FLYWAY_MAVEN_PLUGIN_ARTIFACT: &str = "flyway-maven-plugin";
const FLYWAY_GRADLE_PLUGIN_ID: &str = "org.flywaydb.flyway";

#[derive(Serialize)]
pub struct PrereqsReport {
    pub build_tool: &'static str,
    pub flyway_lib: bool,
    pub flyway_plugin: bool,
    /// True when both Flyway prerequisites (runtime dep + build-tool plugin)
    /// are present. Named `ready` (not `ok`) to avoid colliding with the
    /// `ok` success marker emitted by the JSON envelope.
    pub ready: bool,
    pub missing: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

pub fn run(project_root: &Path) -> Result<()> {
    let report = compute(project_root)?;
    print_json(&report)
}

pub fn compute(project_root: &Path) -> Result<PrereqsReport> {
    let detected = detect(project_root);
    match detected.tool {
        BuildTool::Maven => compute_maven(&detected),
        BuildTool::Gradle => compute_gradle(&detected),
        BuildTool::Unknown => Ok(PrereqsReport {
            build_tool: "unknown",
            flyway_lib: false,
            flyway_plugin: false,
            ready: false,
            missing: vec!["build_file".into()],
            hint: Some(
                "no pom.xml or build.gradle[.kts] found at project root".into(),
            ),
        }),
    }
}

fn compute_maven(detected: &DetectedBuild) -> Result<PrereqsReport> {
    let pom_path = detected
        .pom
        .as_ref()
        .expect("Maven detection guarantees pom path");
    let text = read_pom(pom_path)?;

    let lib_present = match dependencies_span(&text) {
        Some(span) => child_exists(&text, &span, "dependency", |body| {
            child_text(body, "groupId").as_deref() == Some(FLYWAY_GROUP)
                && child_text(body, "artifactId").as_deref() == Some(FLYWAY_LIB_ARTIFACT)
        }),
        None => false,
    };

    let plugin_present = match plugins_span(&text) {
        Some(span) => child_exists(&text, &span, "plugin", |body| {
            child_text(body, "groupId").as_deref() == Some(FLYWAY_GROUP)
                && child_text(body, "artifactId").as_deref() == Some(FLYWAY_MAVEN_PLUGIN_ARTIFACT)
        }),
        None => false,
    };

    Ok(build_report(BuildTool::Maven, lib_present, plugin_present))
}

fn compute_gradle(detected: &DetectedBuild) -> Result<PrereqsReport> {
    let script_path = detected
        .gradle_script
        .as_ref()
        .expect("Gradle detection guarantees script path");
    let text = fs::read_to_string(script_path)?;
    let stripped = strip_gradle_noise(&text);

    let lib_present = stripped.contains(&format!("{FLYWAY_GROUP}:{FLYWAY_LIB_ARTIFACT}"));
    let plugin_present = stripped.contains(FLYWAY_GRADLE_PLUGIN_ID);

    Ok(build_report(BuildTool::Gradle, lib_present, plugin_present))
}

/// Strip `// line comments`, `/* block comments */`, and string literals so
/// we don't false-positive on commented-out or in-string mentions of the
/// Flyway artifact / plugin id. Conservative: when in doubt, keep the byte.
fn strip_gradle_noise(src: &str) -> String {
    let bytes = src.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        // Block comment
        if b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(bytes.len());
            continue;
        }
        // Line comment
        if b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        // String literal: keep contents (the artifact coord usually appears
        // inside `'...'` or `"..."` for `implementation '...'`); we only need
        // to skip comments. So don't strip strings.
        out.push(b as char);
        i += 1;
    }
    out
}

fn build_report(tool: BuildTool, lib: bool, plugin: bool) -> PrereqsReport {
    let mut missing = Vec::new();
    if !lib {
        missing.push("flyway_lib".into());
    }
    if !plugin {
        missing.push("flyway_plugin".into());
    }
    let ready = missing.is_empty();
    let hint = if ready {
        None
    } else {
        Some(hint_for(tool, lib, plugin))
    };
    PrereqsReport {
        build_tool: tool.as_str(),
        flyway_lib: lib,
        flyway_plugin: plugin,
        ready,
        missing,
        hint,
    }
}

fn hint_for(tool: BuildTool, lib: bool, plugin: bool) -> String {
    let mut parts = Vec::new();
    if !lib {
        parts.push("the Flyway runtime dependency (org.flywaydb:flyway-core)");
    }
    if !plugin {
        parts.push(match tool {
            BuildTool::Maven => "the Flyway Maven plugin (org.flywaydb:flyway-maven-plugin)",
            BuildTool::Gradle => "the Flyway Gradle plugin (id 'org.flywaydb.flyway')",
            BuildTool::Unknown => "the Flyway build-tool plugin",
        });
    }
    let what = parts.join(" and ");
    let where_ = match tool {
        BuildTool::Maven => "pom.xml",
        BuildTool::Gradle => "build.gradle[.kts]",
        BuildTool::Unknown => "your build file",
    };
    format!("add {what} to {where_} and re-run")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn write(dir: &Path, name: &str, body: &str) {
        fs::write(dir.join(name), body).unwrap();
    }

    const POM_OK: &str = r#"<project>
  <dependencies>
    <dependency>
      <groupId>org.flywaydb</groupId>
      <artifactId>flyway-core</artifactId>
    </dependency>
  </dependencies>
  <build>
    <plugins>
      <plugin>
        <groupId>org.flywaydb</groupId>
        <artifactId>flyway-maven-plugin</artifactId>
      </plugin>
    </plugins>
  </build>
</project>
"#;

    const POM_MISSING_PLUGIN: &str = r#"<project>
  <dependencies>
    <dependency>
      <groupId>org.flywaydb</groupId>
      <artifactId>flyway-core</artifactId>
    </dependency>
  </dependencies>
</project>
"#;

    const POM_MISSING_BOTH: &str = r#"<project>
  <dependencies>
    <dependency>
      <groupId>org.springframework.boot</groupId>
      <artifactId>spring-boot-starter</artifactId>
    </dependency>
  </dependencies>
</project>
"#;

    #[test]
    fn maven_happy_path() {
        let tmp = tempdir().unwrap();
        write(tmp.path(), "pom.xml", POM_OK);
        let r = compute(tmp.path()).unwrap();
        assert_eq!(r.build_tool, "maven");
        assert!(r.flyway_lib);
        assert!(r.flyway_plugin);
        assert!(r.ready);
        assert!(r.missing.is_empty());
    }

    #[test]
    fn maven_missing_plugin() {
        let tmp = tempdir().unwrap();
        write(tmp.path(), "pom.xml", POM_MISSING_PLUGIN);
        let r = compute(tmp.path()).unwrap();
        assert!(r.flyway_lib);
        assert!(!r.flyway_plugin);
        assert!(!r.ready);
        assert_eq!(r.missing, vec!["flyway_plugin"]);
        assert!(r.hint.is_some());
    }

    #[test]
    fn maven_missing_both() {
        let tmp = tempdir().unwrap();
        write(tmp.path(), "pom.xml", POM_MISSING_BOTH);
        let r = compute(tmp.path()).unwrap();
        assert!(!r.flyway_lib);
        assert!(!r.flyway_plugin);
        assert!(!r.ready);
        assert_eq!(r.missing, vec!["flyway_lib", "flyway_plugin"]);
    }

    const GRADLE_GROOVY_OK: &str = r#"
plugins {
    id 'java'
    id 'org.flywaydb.flyway' version '10.0.0'
}
dependencies {
    implementation 'org.flywaydb:flyway-core'
    implementation 'org.springframework.boot:spring-boot-starter'
}
"#;

    const GRADLE_KOTLIN_OK: &str = r#"
plugins {
    java
    id("org.flywaydb.flyway") version "10.0.0"
}
dependencies {
    implementation("org.flywaydb:flyway-core")
}
"#;

    const GRADLE_COMMENTED_OUT: &str = r#"
plugins {
    id 'java'
    // id 'org.flywaydb.flyway' version '10.0.0'
}
dependencies {
    /* implementation 'org.flywaydb:flyway-core' */
}
"#;

    #[test]
    fn gradle_groovy_happy_path() {
        let tmp = tempdir().unwrap();
        write(tmp.path(), "build.gradle", GRADLE_GROOVY_OK);
        let r = compute(tmp.path()).unwrap();
        assert_eq!(r.build_tool, "gradle");
        assert!(r.flyway_lib);
        assert!(r.flyway_plugin);
        assert!(r.ready);
    }

    #[test]
    fn gradle_kotlin_happy_path() {
        let tmp = tempdir().unwrap();
        write(tmp.path(), "build.gradle.kts", GRADLE_KOTLIN_OK);
        let r = compute(tmp.path()).unwrap();
        assert_eq!(r.build_tool, "gradle");
        assert!(r.flyway_lib);
        assert!(r.flyway_plugin);
        assert!(r.ready);
    }

    #[test]
    fn gradle_commented_out_does_not_count() {
        let tmp = tempdir().unwrap();
        write(tmp.path(), "build.gradle", GRADLE_COMMENTED_OUT);
        let r = compute(tmp.path()).unwrap();
        assert!(!r.flyway_lib);
        assert!(!r.flyway_plugin);
        assert!(!r.ready);
    }

    #[test]
    fn unknown_when_no_build_file() {
        let tmp = tempdir().unwrap();
        let r = compute(tmp.path()).unwrap();
        assert_eq!(r.build_tool, "unknown");
        assert!(!r.ready);
        assert_eq!(r.missing, vec!["build_file"]);
    }
}
