//! Path resolution for `docs/domains/<slug>/` content. Owns the
//! single-type-stays-flat / multi-type-uses-subdirs decision so every consumer
//! (`scaffold-docs`, `drift`, `scenarios gap`, etc.) agrees on the layout.
//!
//! ## Layout rule
//!
//! When a domain exposes a single API type, files stay flat:
//!
//! ```text
//! docs/domains/<slug>/api-spec.yaml
//! docs/domains/<slug>/api-implement-logic.md
//! docs/domains/<slug>/test-scenarios.yaml
//! docs/domains/<slug>/domain-model.md
//! docs/domains/<slug>/conflicts.md
//! ```
//!
//! When a domain exposes ≥2 API types, the per-type files move into subdirs:
//!
//! ```text
//! docs/domains/<slug>/web-api/api-spec.yaml
//! docs/domains/<slug>/web-api/api-implement-logic.md
//! docs/domains/<slug>/web-api/test-scenarios.yaml
//! docs/domains/<slug>/microservice-api/api-spec.yaml
//! docs/domains/<slug>/microservice-api/api-implement-logic.md
//! docs/domains/<slug>/microservice-api/test-scenarios.yaml
//! docs/domains/<slug>/domain-model.md       (still shared)
//! docs/domains/<slug>/conflicts.md          (still shared)
//! ```

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ApiType {
    WebApi,
    MicroserviceApi,
    OpenApi,
}

impl ApiType {
    pub fn dir_name(self) -> &'static str {
        match self {
            ApiType::WebApi => "web-api",
            ApiType::MicroserviceApi => "microservice-api",
            ApiType::OpenApi => "open-api",
        }
    }

    pub fn from_dir_name(s: &str) -> Option<Self> {
        match s {
            "web-api" => Some(ApiType::WebApi),
            "microservice-api" => Some(ApiType::MicroserviceApi),
            "open-api" => Some(ApiType::OpenApi),
            _ => None,
        }
    }

    pub fn all() -> &'static [ApiType] {
        &[ApiType::WebApi, ApiType::MicroserviceApi, ApiType::OpenApi]
    }
}

/// Per-API-type file paths within a single domain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PerTypePaths {
    pub api_spec: PathBuf,
    pub implement_logic: PathBuf,
    pub scenarios: PathBuf,
}

/// All file paths for a single domain, computed once given the set of API
/// types present. Pass through to writers/readers; they should never join
/// paths themselves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DomainPaths {
    pub root: PathBuf,
    pub domain_model: PathBuf,
    pub conflicts: PathBuf,
    /// One entry per API type detected for this domain. Index by `ApiType`.
    pub per_type: std::collections::BTreeMap<ApiType, PerTypePaths>,
    /// `true` when ≥2 API types are present and subdirectories are in use.
    pub multi_type: bool,
}

impl DomainPaths {
    /// Path to the API-type subdirectory when `multi_type`, else the domain
    /// root. Useful for callers that need to create the directory before
    /// writing files into it.
    pub fn dir_for(&self, api_type: ApiType) -> PathBuf {
        if self.multi_type {
            self.root.join(api_type.dir_name())
        } else {
            self.root.clone()
        }
    }
}

/// Compute paths for a domain given the API types present. `api_types` should
/// be deduplicated; an empty slice is treated as "single-type, flat" with no
/// `per_type` entries (caller should normally provide at least one).
pub fn paths_for(
    domains_root: &Path,
    slug: &str,
    api_types: &[ApiType],
) -> DomainPaths {
    let root = domains_root.join(slug);
    let multi_type = api_types.len() > 1;
    let mut per_type = std::collections::BTreeMap::new();
    for ty in api_types {
        let prefix = if multi_type {
            root.join(ty.dir_name())
        } else {
            root.clone()
        };
        per_type.insert(
            *ty,
            PerTypePaths {
                api_spec: prefix.join("api-spec.yaml"),
                implement_logic: prefix.join("api-implement-logic.md"),
                scenarios: prefix.join("test-scenarios.yaml"),
            },
        );
    }
    DomainPaths {
        root: root.clone(),
        domain_model: root.join("domain-model.md"),
        conflicts: root.join("conflicts.md"),
        per_type,
        multi_type,
    }
}

/// What a reader sees on disk for a domain. Readers (`drift`, `scenarios
/// gap`) call this first, then read whichever paths are present.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DetectedLayout {
    /// No domain directory at all.
    Missing,
    /// Domain dir exists but has no recognizable api-spec / API-type subdirs.
    Empty,
    /// `docs/domains/<slug>/api-spec.yaml` is present (no API-type subdirs).
    /// The single API type isn't directly identifiable from disk — readers
    /// should treat the flat file as "the one API type for this domain".
    Flat,
    /// One or more API-type subdirs (`web-api/`, etc.) are present, each with
    /// its own files.
    MultiType(Vec<ApiType>),
}

pub fn detect_layout(domains_root: &Path, slug: &str) -> DetectedLayout {
    let root = domains_root.join(slug);
    if !root.exists() {
        return DetectedLayout::Missing;
    }

    let mut present_types = Vec::new();
    for ty in ApiType::all() {
        let sub = root.join(ty.dir_name());
        if sub.is_dir() && sub.join("api-spec.yaml").exists() {
            present_types.push(*ty);
        }
    }
    if !present_types.is_empty() {
        return DetectedLayout::MultiType(present_types);
    }

    if root.join("api-spec.yaml").exists() {
        DetectedLayout::Flat
    } else {
        DetectedLayout::Empty
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn flat_when_single_type() {
        let p = paths_for(Path::new("docs/domains"), "billing", &[ApiType::WebApi]);
        assert!(!p.multi_type);
        let pt = &p.per_type[&ApiType::WebApi];
        assert_eq!(pt.api_spec, Path::new("docs/domains/billing/api-spec.yaml"));
        assert_eq!(p.domain_model, Path::new("docs/domains/billing/domain-model.md"));
        assert_eq!(p.dir_for(ApiType::WebApi), Path::new("docs/domains/billing"));
    }

    #[test]
    fn nested_when_multi_type() {
        let p = paths_for(
            Path::new("docs/domains"),
            "billing",
            &[ApiType::WebApi, ApiType::MicroserviceApi],
        );
        assert!(p.multi_type);
        assert_eq!(
            p.per_type[&ApiType::WebApi].api_spec,
            Path::new("docs/domains/billing/web-api/api-spec.yaml")
        );
        assert_eq!(
            p.per_type[&ApiType::MicroserviceApi].api_spec,
            Path::new("docs/domains/billing/microservice-api/api-spec.yaml")
        );
        // domain-model + conflicts always shared at the root.
        assert_eq!(
            p.domain_model,
            Path::new("docs/domains/billing/domain-model.md")
        );
        assert_eq!(p.conflicts, Path::new("docs/domains/billing/conflicts.md"));
        assert_eq!(
            p.dir_for(ApiType::WebApi),
            Path::new("docs/domains/billing/web-api")
        );
    }

    #[test]
    fn detect_missing() {
        let dir = TempDir::new().unwrap();
        assert_eq!(
            detect_layout(dir.path(), "billing"),
            DetectedLayout::Missing
        );
    }

    #[test]
    fn detect_empty_when_dir_present_but_no_specs() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("billing")).unwrap();
        assert_eq!(detect_layout(dir.path(), "billing"), DetectedLayout::Empty);
    }

    #[test]
    fn detect_flat_when_root_api_spec_present() {
        let dir = TempDir::new().unwrap();
        let domain = dir.path().join("billing");
        fs::create_dir_all(&domain).unwrap();
        fs::write(domain.join("api-spec.yaml"), "openapi: 3.0.3").unwrap();
        assert_eq!(detect_layout(dir.path(), "billing"), DetectedLayout::Flat);
    }

    #[test]
    fn detect_multi_type_when_subdirs_present() {
        let dir = TempDir::new().unwrap();
        let domain = dir.path().join("billing");
        fs::create_dir_all(domain.join("web-api")).unwrap();
        fs::write(domain.join("web-api").join("api-spec.yaml"), "x").unwrap();
        fs::create_dir_all(domain.join("microservice-api")).unwrap();
        fs::write(domain.join("microservice-api").join("api-spec.yaml"), "y").unwrap();
        let layout = detect_layout(dir.path(), "billing");
        if let DetectedLayout::MultiType(types) = layout {
            assert!(types.contains(&ApiType::WebApi));
            assert!(types.contains(&ApiType::MicroserviceApi));
            assert!(!types.contains(&ApiType::OpenApi));
        } else {
            panic!("expected MultiType, got {:?}", layout);
        }
    }

    #[test]
    fn api_type_round_trips_dir_names() {
        for ty in ApiType::all() {
            assert_eq!(ApiType::from_dir_name(ty.dir_name()), Some(*ty));
        }
        assert_eq!(ApiType::from_dir_name("nonsense"), None);
    }

    #[test]
    fn flat_layout_takes_precedence_check() {
        // A directory that has BOTH a flat api-spec.yaml AND a subdir with
        // api-spec.yaml should report MultiType (subdirs are stronger signal —
        // flat output by scaffold-docs would have removed the subdirs).
        let dir = TempDir::new().unwrap();
        let domain = dir.path().join("billing");
        fs::create_dir_all(domain.join("web-api")).unwrap();
        fs::write(domain.join("web-api").join("api-spec.yaml"), "x").unwrap();
        fs::write(domain.join("api-spec.yaml"), "y").unwrap();
        let layout = detect_layout(dir.path(), "billing");
        match layout {
            DetectedLayout::MultiType(_) => {}
            other => panic!("expected MultiType, got {:?}", other),
        }
    }
}
