//! Load/save for `docs/domains.yaml`.
//!
//! Round-trips unknown top-level keys (e.g., `service:`) and any per-domain
//! fields we don't recognize, so the file format stays open to extension by
//! humans even though `domains add/set-*` only touches `description` and
//! `use_when`. The doctor surfaces unknown per-domain keys as warnings.

use anyhow::{Context, Result};
use serde_yaml::{Mapping, Value};
use std::fs;
use std::path::{Path, PathBuf};

pub const FILE_RELATIVE: &str = "docs/domains.yaml";
pub const DESCRIPTION_MAX: usize = 120;
pub const USE_WHEN_MAX: usize = 140;

#[derive(Debug, Clone)]
pub struct DomainEntry {
    pub description: Option<String>,
    pub use_when: Option<String>,
    /// Other raw keys preserved verbatim on round-trip.
    pub extras: Vec<(String, Value)>,
}

impl DomainEntry {
    pub fn new(description: &str, use_when: &str) -> Self {
        Self {
            description: Some(description.to_string()),
            use_when: Some(use_when.to_string()),
            extras: Vec::new(),
        }
    }
}

pub struct DomainsFile {
    pub path: PathBuf,
    /// Top-level mapping; carries `service:` and any future top-level fields
    /// untouched. The `domains:` key is also stored here as a Mapping; we read
    /// and rewrite it through the typed helpers below.
    top: Mapping,
}

impl DomainsFile {
    /// Load `docs/domains.yaml` from `project_root`. If the file does not
    /// exist, return an empty in-memory document; callers that need to
    /// distinguish absent from empty should check `path.exists()` themselves.
    pub fn load(project_root: &Path) -> Result<Self, LoadError> {
        let path = project_root.join(FILE_RELATIVE);
        if !path.exists() {
            return Ok(Self {
                path,
                top: Mapping::new(),
            });
        }
        let text = fs::read_to_string(&path).map_err(|e| LoadError::Io {
            path: path.clone(),
            source: e,
        })?;
        let parsed: Value = serde_yaml::from_str(&text).map_err(|e| LoadError::Malformed {
            path: path.clone(),
            message: e.to_string(),
        })?;
        let top = match parsed {
            Value::Null => Mapping::new(),
            Value::Mapping(m) => m,
            _ => {
                return Err(LoadError::Malformed {
                    path,
                    message: "expected a YAML mapping at the top level".to_string(),
                })
            }
        };
        Ok(Self { path, top })
    }

    /// Persist the document. Creates parent directories as needed.
    pub fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let s = serde_yaml::to_string(&Value::Mapping(self.top.clone()))
            .context("serializing docs/domains.yaml")?;
        crate::util::atomic_write(&self.path, s.as_bytes())
    }

    /// Borrow the `domains:` sub-mapping. Returns an empty mapping if the
    /// key is absent or holds null.
    fn domains_map(&self) -> Mapping {
        match self.top.get(Value::String("domains".into())) {
            Some(Value::Mapping(m)) => m.clone(),
            _ => Mapping::new(),
        }
    }

    fn put_domains_map(&mut self, m: Mapping) {
        self.top
            .insert(Value::String("domains".into()), Value::Mapping(m));
    }

    /// Iterate domain entries in file order. Slug keys that are not strings
    /// are silently skipped; the doctor surfaces them as malformed.
    pub fn entries(&self) -> Vec<(String, DomainEntry)> {
        let m = self.domains_map();
        m.into_iter()
            .filter_map(|(k, v)| {
                let slug = k.as_str()?.to_string();
                Some((slug, parse_entry(&v)))
            })
            .collect()
    }

    pub fn get(&self, slug: &str) -> Option<DomainEntry> {
        let m = self.domains_map();
        m.get(Value::String(slug.into())).map(parse_entry)
    }

    pub fn upsert(&mut self, slug: &str, entry: DomainEntry) {
        let mut domains = self.domains_map();
        let existing = domains
            .get(Value::String(slug.into()))
            .cloned()
            .unwrap_or(Value::Mapping(Mapping::new()));
        let mut node = match existing {
            Value::Mapping(m) => m,
            _ => Mapping::new(),
        };
        write_field(&mut node, "description", entry.description.as_deref());
        write_field(&mut node, "use_when", entry.use_when.as_deref());
        for (k, v) in entry.extras {
            node.insert(Value::String(k), v);
        }
        domains.insert(Value::String(slug.into()), Value::Mapping(node));
        self.put_domains_map(domains);
    }

    /// Update a single field. Returns `Err(NotFound)` if the slug is absent.
    pub fn set_description(&mut self, slug: &str, text: &str) -> Result<(), NotFound> {
        self.set_field(slug, "description", text)
    }

    pub fn set_use_when(&mut self, slug: &str, text: &str) -> Result<(), NotFound> {
        self.set_field(slug, "use_when", text)
    }

    fn set_field(&mut self, slug: &str, field: &str, text: &str) -> Result<(), NotFound> {
        let mut domains = self.domains_map();
        let key = Value::String(slug.into());
        let node = match domains.get_mut(&key) {
            Some(Value::Mapping(m)) => m,
            _ => return Err(NotFound(slug.to_string())),
        };
        node.insert(
            Value::String(field.to_string()),
            Value::String(text.to_string()),
        );
        self.put_domains_map(domains);
        Ok(())
    }
}

fn parse_entry(v: &Value) -> DomainEntry {
    match v {
        Value::Mapping(m) => {
            let mut description = None;
            let mut use_when = None;
            let mut extras = Vec::new();
            for (k, val) in m {
                let key = match k.as_str() {
                    Some(s) => s,
                    None => continue,
                };
                match key {
                    "description" => description = val.as_str().map(str::to_string),
                    "use_when" => use_when = val.as_str().map(str::to_string),
                    other => extras.push((other.to_string(), val.clone())),
                }
            }
            DomainEntry {
                description,
                use_when,
                extras,
            }
        }
        _ => DomainEntry {
            description: None,
            use_when: None,
            extras: Vec::new(),
        },
    }
}

fn write_field(node: &mut Mapping, key: &str, value: Option<&str>) {
    if let Some(v) = value {
        node.insert(
            Value::String(key.to_string()),
            Value::String(v.to_string()),
        );
    }
}

#[derive(Debug)]
pub struct NotFound(pub String);

impl std::fmt::Display for NotFound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "domain `{}` not found in docs/domains.yaml", self.0)
    }
}

impl std::error::Error for NotFound {}

#[derive(Debug)]
pub enum LoadError {
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    Malformed {
        path: PathBuf,
        message: String,
    },
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::Io { path, source } => write!(f, "reading {}: {}", path.display(), source),
            LoadError::Malformed { path, message } => {
                write!(f, "{} is malformed YAML: {}", path.display(), message)
            }
        }
    }
}

impl std::error::Error for LoadError {}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn write(p: &Path, s: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, s).unwrap();
    }

    #[test]
    fn load_absent_file_yields_empty_doc() {
        let tmp = tempdir().unwrap();
        let f = DomainsFile::load(tmp.path()).unwrap();
        assert!(f.entries().is_empty());
    }

    #[test]
    fn malformed_yaml_surfaces_loaderror_malformed() {
        let tmp = tempdir().unwrap();
        write(&tmp.path().join("docs/domains.yaml"), ":\n  - not-a-mapping\n");
        match DomainsFile::load(tmp.path()) {
            Err(LoadError::Malformed { .. }) => {}
            Err(other) => panic!("expected Malformed, got {other}"),
            Ok(_) => panic!("expected Malformed, got Ok"),
        }
    }

    #[test]
    fn upsert_then_load_round_trips_description_and_use_when() {
        let tmp = tempdir().unwrap();
        let mut f = DomainsFile::load(tmp.path()).unwrap();
        f.upsert("billing", DomainEntry::new("invoices", "for invoicing"));
        f.save().unwrap();
        let f2 = DomainsFile::load(tmp.path()).unwrap();
        let entry = f2.get("billing").unwrap();
        assert_eq!(entry.description.as_deref(), Some("invoices"));
        assert_eq!(entry.use_when.as_deref(), Some("for invoicing"));
    }

    #[test]
    fn extras_and_top_level_unknown_keys_are_preserved() {
        let tmp = tempdir().unwrap();
        write(
            &tmp.path().join("docs/domains.yaml"),
            r#"service: order-service
domains:
  billing:
    description: invoices
    use_when: for invoicing
    keywords: [a, b]
"#,
        );
        let mut f = DomainsFile::load(tmp.path()).unwrap();
        f.set_description("billing", "updated invoices").unwrap();
        f.save().unwrap();
        let text = std::fs::read_to_string(tmp.path().join("docs/domains.yaml")).unwrap();
        assert!(text.contains("service: order-service"));
        assert!(text.contains("keywords"));
        assert!(text.contains("updated invoices"));
    }

    #[test]
    fn set_field_on_unknown_slug_returns_not_found() {
        let tmp = tempdir().unwrap();
        let mut f = DomainsFile::load(tmp.path()).unwrap();
        let err = f.set_description("nope", "x").unwrap_err();
        assert_eq!(err.0, "nope");
    }
}
