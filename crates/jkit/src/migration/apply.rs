use anyhow::Result;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::build_tool::{detect, BuildTool};
use crate::util::print_json;

const STDOUT_TAIL_LINES: usize = 50;
const STDERR_TAIL_LINES: usize = 50;
const TAIL_BYTE_CAP: usize = 4096;

#[derive(Serialize)]
pub struct ApplyReport {
    pub applied: bool,
    pub build_tool: &'static str,
    /// The exact command string we invoked, for debugging.
    pub command: String,
    pub exit_code: Option<i32>,
    pub stdout_tail: String,
    pub stderr_tail: String,
}

pub fn run(project_root: &Path) -> Result<()> {
    let detected = detect(project_root);
    let (program, args, label) = match detected.tool {
        BuildTool::Maven => ("mvn".to_string(), vec!["-q".to_string(), "flyway:migrate".to_string()], BuildTool::Maven),
        BuildTool::Gradle => {
            let (program, args) = gradle_invocation(project_root);
            (program, args, BuildTool::Gradle)
        }
        BuildTool::Unknown => {
            anyhow::bail!(
                "no Maven (pom.xml) or Gradle (build.gradle[.kts]) build file found at {}; cannot apply migration",
                project_root.display()
            );
        }
    };

    // Pre-flight: confirm the executable is reachable. Resolves bare names
    // through PATH; absolute paths (e.g. ./gradlew) are checked as files.
    if !is_invocable(&program, project_root) {
        let hint = match label {
            BuildTool::Maven => "install Maven and ensure `mvn` is on PATH",
            BuildTool::Gradle => "install Gradle (or commit the Gradle wrapper) and ensure it is on PATH",
            BuildTool::Unknown => "unreachable",
        };
        anyhow::bail!("{program} not found: {hint}");
    }

    let command_str = format!("{} {}", program, args.join(" "));
    let output = Command::new(&program)
        .args(&args)
        .current_dir(project_root)
        .output()
        .map_err(|e| anyhow::anyhow!("spawning `{command_str}` failed: {e}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let exit_code = output.status.code();
    let applied = output.status.success();

    let report = ApplyReport {
        applied,
        build_tool: label.as_str(),
        command: command_str,
        exit_code,
        stdout_tail: tail(&stdout, STDOUT_TAIL_LINES, TAIL_BYTE_CAP),
        stderr_tail: tail(&stderr, STDERR_TAIL_LINES, TAIL_BYTE_CAP),
    };
    print_json(&report)
}

/// Choose the Gradle entry point: prefer `./gradlew` when committed in the
/// project root, otherwise fall back to `gradle` on PATH.
fn gradle_invocation(project_root: &Path) -> (String, Vec<String>) {
    let wrapper = project_root.join("gradlew");
    if wrapper.is_file() {
        // Run via absolute path so `current_dir` doesn't fight `./` semantics.
        let abs = wrapper
            .canonicalize()
            .unwrap_or_else(|_| wrapper.clone());
        (
            abs.to_string_lossy().into_owned(),
            vec!["flywayMigrate".to_string()],
        )
    } else {
        (
            "gradle".to_string(),
            vec!["flywayMigrate".to_string()],
        )
    }
}

fn is_invocable(program: &str, project_root: &Path) -> bool {
    let path = PathBuf::from(program);
    if path.is_absolute() || path.components().count() > 1 {
        path.is_file()
    } else {
        which::which_in(program, std::env::var_os("PATH"), project_root).is_ok()
    }
}

/// Return the last `max_lines` lines of `text`, then trim from the front so
/// the result is at most `max_bytes` bytes (UTF-8 boundary-safe).
fn tail(text: &str, max_lines: usize, max_bytes: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(max_lines);
    let mut out = lines[start..].join("\n");
    if out.len() > max_bytes {
        let mut cut = out.len() - max_bytes;
        while cut < out.len() && !out.is_char_boundary(cut) {
            cut += 1;
        }
        out = out[cut..].to_string();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn tail_returns_last_n_lines() {
        let text = "a\nb\nc\nd\ne";
        assert_eq!(tail(text, 3, 1024), "c\nd\ne");
    }

    #[test]
    fn tail_caps_byte_length() {
        let big = "x".repeat(10_000);
        let out = tail(&big, 50, 100);
        assert!(out.len() <= 100);
    }

    #[test]
    fn tail_handles_short_input() {
        assert_eq!(tail("only line", 50, 1024), "only line");
        assert_eq!(tail("", 50, 1024), "");
    }

    #[test]
    fn gradle_invocation_prefers_wrapper() {
        let tmp = tempdir().unwrap();
        fs::write(tmp.path().join("gradlew"), "#!/bin/sh\nexit 0\n").unwrap();
        let (prog, args) = gradle_invocation(tmp.path());
        assert!(prog.ends_with("gradlew"));
        assert_eq!(args, vec!["flywayMigrate"]);
    }

    #[test]
    fn gradle_invocation_falls_back_to_path() {
        let tmp = tempdir().unwrap();
        let (prog, args) = gradle_invocation(tmp.path());
        assert_eq!(prog, "gradle");
        assert_eq!(args, vec!["flywayMigrate"]);
    }
}
