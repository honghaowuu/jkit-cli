use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BuildTool {
    Maven,
    Gradle,
    Unknown,
}

impl BuildTool {
    pub fn as_str(&self) -> &'static str {
        match self {
            BuildTool::Maven => "maven",
            BuildTool::Gradle => "gradle",
            BuildTool::Unknown => "unknown",
        }
    }
}

/// Detected build files for the project. Maven wins when both are present
/// (jkit's pipeline is Maven-first), but the Gradle paths are still surfaced
/// so callers can warn about a mixed-tool project if they care.
pub struct DetectedBuild {
    pub tool: BuildTool,
    /// Maven `pom.xml` path when present.
    pub pom: Option<PathBuf>,
    /// Gradle Groovy or Kotlin DSL build script path when present.
    pub gradle_script: Option<PathBuf>,
}

pub fn detect(project_root: &Path) -> DetectedBuild {
    let pom = project_root.join("pom.xml");
    let gradle_groovy = project_root.join("build.gradle");
    let gradle_kotlin = project_root.join("build.gradle.kts");

    let pom_path = if pom.is_file() { Some(pom) } else { None };
    let gradle_path = if gradle_groovy.is_file() {
        Some(gradle_groovy)
    } else if gradle_kotlin.is_file() {
        Some(gradle_kotlin)
    } else {
        None
    };

    let tool = match (&pom_path, &gradle_path) {
        (Some(_), _) => BuildTool::Maven,
        (None, Some(_)) => BuildTool::Gradle,
        (None, None) => BuildTool::Unknown,
    };

    DetectedBuild {
        tool,
        pom: pom_path,
        gradle_script: gradle_path,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn detect_maven_when_pom_present() {
        let tmp = tempdir().unwrap();
        fs::write(tmp.path().join("pom.xml"), "<project/>").unwrap();
        let d = detect(tmp.path());
        assert_eq!(d.tool, BuildTool::Maven);
        assert!(d.pom.is_some());
    }

    #[test]
    fn detect_gradle_groovy() {
        let tmp = tempdir().unwrap();
        fs::write(tmp.path().join("build.gradle"), "").unwrap();
        let d = detect(tmp.path());
        assert_eq!(d.tool, BuildTool::Gradle);
        assert!(d.gradle_script.is_some());
    }

    #[test]
    fn detect_gradle_kotlin() {
        let tmp = tempdir().unwrap();
        fs::write(tmp.path().join("build.gradle.kts"), "").unwrap();
        let d = detect(tmp.path());
        assert_eq!(d.tool, BuildTool::Gradle);
    }

    #[test]
    fn detect_unknown_when_no_build_file() {
        let tmp = tempdir().unwrap();
        let d = detect(tmp.path());
        assert_eq!(d.tool, BuildTool::Unknown);
    }

    #[test]
    fn maven_wins_when_both_present() {
        let tmp = tempdir().unwrap();
        fs::write(tmp.path().join("pom.xml"), "<project/>").unwrap();
        fs::write(tmp.path().join("build.gradle"), "").unwrap();
        let d = detect(tmp.path());
        assert_eq!(d.tool, BuildTool::Maven);
    }
}
