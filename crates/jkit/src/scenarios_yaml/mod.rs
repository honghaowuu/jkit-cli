//! Load/save for the top-level `docs/test-scenarios.yaml`.
//!
//! ## Format
//!
//! ```yaml
//! domains:
//!   billing:                    # single-API-type domain — flat list
//!     - endpoint: POST /api/billing/invoices
//!       id: bulk-invoice-happy-path
//!       description: ...
//!   payment:                    # multi-API-type domain — keyed by api-type
//!     web-api:
//!       - endpoint: ...
//!         id: ...
//!         description: ...
//!     microservice-api:
//!       - endpoint: ...
//!         id: ...
//!         description: ...
//! ```
//!
//! Unknown top-level keys are preserved on round-trip; per-slug entries are
//! preserved verbatim except where this module updates them.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_yaml::{Mapping, Value};
use std::fs;
use std::path::{Path, PathBuf};

pub const FILE_RELATIVE: &str = "docs/test-scenarios.yaml";

/// One scenario entry as it appears in `test-scenarios.yaml`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScenarioEntry {
    pub endpoint: String,
    pub id: String,
    pub description: String,
}

/// Per-slug shape: either a flat list (single API type) or a map of api-type
/// to entries (multi-type domain).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlugSection {
    Flat(Vec<ScenarioEntry>),
    PerApiType(Vec<(String, Vec<ScenarioEntry>)>),
}

impl SlugSection {
    /// All entries flattened. For `PerApiType`, entries from each bucket are
    /// emitted in declaration order, paired with their api-type. For `Flat`,
    /// each entry is paired with `None`.
    pub fn iter_with_type(&self) -> Box<dyn Iterator<Item = (Option<&str>, &ScenarioEntry)> + '_> {
        match self {
            SlugSection::Flat(v) => Box::new(v.iter().map(|e| (None, e))),
            SlugSection::PerApiType(buckets) => Box::new(
                buckets
                    .iter()
                    .flat_map(|(ty, v)| v.iter().map(move |e| (Some(ty.as_str()), e))),
            ),
        }
    }

    pub fn is_empty(&self) -> bool {
        match self {
            SlugSection::Flat(v) => v.is_empty(),
            SlugSection::PerApiType(buckets) => buckets.iter().all(|(_, v)| v.is_empty()),
        }
    }
}

pub struct ScenariosFile {
    pub path: PathBuf,
    top: Mapping,
}

impl ScenariosFile {
    /// Load `docs/test-scenarios.yaml` from `project_root`. Absent file
    /// yields an empty in-memory document.
    pub fn load(project_root: &Path) -> Result<Self> {
        let path = project_root.join(FILE_RELATIVE);
        if !path.exists() {
            return Ok(Self {
                path,
                top: Mapping::new(),
            });
        }
        let text = fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let parsed: Value = if text.trim().is_empty() {
            Value::Mapping(Mapping::new())
        } else {
            serde_yaml::from_str(&text)
                .with_context(|| format!("parsing {} as YAML", path.display()))?
        };
        let top = match parsed {
            Value::Null => Mapping::new(),
            Value::Mapping(m) => m,
            _ => anyhow::bail!(
                "{}: expected a YAML mapping at the top level",
                path.display()
            ),
        };
        Ok(Self { path, top })
    }

    pub fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let s = serde_yaml::to_string(&Value::Mapping(self.top.clone()))
            .context("serializing docs/test-scenarios.yaml")?;
        crate::util::atomic_write(&self.path, s.as_bytes())
    }

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

    /// Slugs present under `domains:` in the file, in file order.
    pub fn slugs(&self) -> Vec<String> {
        self.domains_map()
            .into_iter()
            .filter_map(|(k, _)| k.as_str().map(str::to_string))
            .collect()
    }

    /// Fetch the section for a slug, decoding flat-vs-multi shape.
    /// `Ok(None)` when the slug is absent. Errors when the section is
    /// malformed (e.g., a string instead of list/map).
    pub fn section(&self, slug: &str) -> Result<Option<SlugSection>> {
        let m = self.domains_map();
        let raw = match m.get(Value::String(slug.into())) {
            Some(v) => v.clone(),
            None => return Ok(None),
        };
        match raw {
            Value::Null => Ok(Some(SlugSection::Flat(Vec::new()))),
            Value::Sequence(seq) => Ok(Some(SlugSection::Flat(parse_entries(&seq, slug, None)?))),
            Value::Mapping(buckets) => {
                let mut out = Vec::new();
                for (k, v) in buckets {
                    let ty = match k.as_str() {
                        Some(s) => s.to_string(),
                        None => anyhow::bail!(
                            "{}: domains.{slug} has a non-string api-type key",
                            FILE_RELATIVE
                        ),
                    };
                    let seq = match v {
                        Value::Sequence(s) => s,
                        Value::Null => Vec::new(),
                        _ => anyhow::bail!(
                            "{}: domains.{slug}.{ty} must be a list of scenario entries",
                            FILE_RELATIVE
                        ),
                    };
                    out.push((ty.clone(), parse_entries(&seq, slug, Some(&ty))?));
                }
                Ok(Some(SlugSection::PerApiType(out)))
            }
            _ => anyhow::bail!(
                "{}: domains.{slug} must be either a list (single api-type) or mapping of api-type to list",
                FILE_RELATIVE
            ),
        }
    }

    /// Replace the entries for a slug. `api_type=None` writes a flat list;
    /// `Some(t)` updates one bucket of the per-api-type mapping (creating the
    /// mapping if the slug was absent or flat-empty).
    ///
    /// Refuses to convert an existing flat-with-entries section to per-api-type
    /// or vice versa — callers asking for a layout switch should explicitly
    /// rewrite the section first.
    pub fn put_entries(
        &mut self,
        slug: &str,
        api_type: Option<&str>,
        entries: &[ScenarioEntry],
    ) -> Result<()> {
        let mut domains = self.domains_map();
        let key = Value::String(slug.into());
        let new_value = match (api_type, domains.get(&key).cloned()) {
            (None, _) => Value::Sequence(entries.iter().map(entry_to_value).collect()),
            (Some(ty), existing) => {
                let mut buckets = match existing {
                    Some(Value::Mapping(m)) => m,
                    Some(Value::Sequence(seq)) if seq.is_empty() => Mapping::new(),
                    None | Some(Value::Null) => Mapping::new(),
                    Some(Value::Sequence(_)) => anyhow::bail!(
                        "domains.{slug} is currently a flat list with entries; \
cannot append api-type-keyed entries without an explicit layout migration"
                    ),
                    _ => anyhow::bail!("domains.{slug} is malformed; cannot update"),
                };
                buckets.insert(
                    Value::String(ty.into()),
                    Value::Sequence(entries.iter().map(entry_to_value).collect()),
                );
                Value::Mapping(buckets)
            }
        };
        domains.insert(key, new_value);
        self.put_domains_map(domains);
        Ok(())
    }

    /// Idempotent: ensure a `domains.<slug>` key exists. If absent, seed it
    /// with an empty flat list (`[]`). If present (any shape), leave it.
    pub fn ensure_slug_seeded(&mut self, slug: &str) {
        let mut domains = self.domains_map();
        let key = Value::String(slug.into());
        if !domains.contains_key(&key) {
            domains.insert(key, Value::Sequence(Vec::new()));
            self.put_domains_map(domains);
        }
    }
}

fn parse_entries(
    seq: &[Value],
    slug: &str,
    api_type: Option<&str>,
) -> Result<Vec<ScenarioEntry>> {
    let mut out = Vec::with_capacity(seq.len());
    for (i, v) in seq.iter().enumerate() {
        let entry: ScenarioEntry = serde_yaml::from_value(v.clone()).with_context(|| {
            let where_ = match api_type {
                Some(t) => format!("domains.{slug}.{t}[{i}]"),
                None => format!("domains.{slug}[{i}]"),
            };
            format!("{}: malformed entry at {where_}", FILE_RELATIVE)
        })?;
        out.push(entry);
    }
    Ok(out)
}

fn entry_to_value(e: &ScenarioEntry) -> Value {
    let mut m = Mapping::new();
    m.insert(
        Value::String("endpoint".into()),
        Value::String(e.endpoint.clone()),
    );
    m.insert(Value::String("id".into()), Value::String(e.id.clone()));
    m.insert(
        Value::String("description".into()),
        Value::String(e.description.clone()),
    );
    Value::Mapping(m)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn write(p: &Path, s: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, s).unwrap();
    }

    #[test]
    fn absent_file_loads_empty() {
        let tmp = tempdir().unwrap();
        let f = ScenariosFile::load(tmp.path()).unwrap();
        assert!(f.slugs().is_empty());
    }

    #[test]
    fn parses_flat_single_type() {
        let tmp = tempdir().unwrap();
        write(
            &tmp.path().join("docs/test-scenarios.yaml"),
            r#"domains:
  billing:
    - endpoint: POST /a
      id: happy-path
      description: ok
"#,
        );
        let f = ScenariosFile::load(tmp.path()).unwrap();
        let s = f.section("billing").unwrap().unwrap();
        match s {
            SlugSection::Flat(v) => {
                assert_eq!(v.len(), 1);
                assert_eq!(v[0].id, "happy-path");
            }
            _ => panic!("expected flat"),
        }
    }

    #[test]
    fn parses_per_api_type_multi() {
        let tmp = tempdir().unwrap();
        write(
            &tmp.path().join("docs/test-scenarios.yaml"),
            r#"domains:
  payment:
    web-api:
      - endpoint: POST /w
        id: a
        description: ok
    microservice-api:
      - endpoint: POST /m
        id: b
        description: ok
"#,
        );
        let f = ScenariosFile::load(tmp.path()).unwrap();
        let s = f.section("payment").unwrap().unwrap();
        match s {
            SlugSection::PerApiType(buckets) => {
                let types: Vec<_> = buckets.iter().map(|(t, _)| t.as_str()).collect();
                assert!(types.contains(&"web-api"));
                assert!(types.contains(&"microservice-api"));
            }
            _ => panic!("expected per-api-type"),
        }
    }

    #[test]
    fn put_flat_then_load_round_trips() {
        let tmp = tempdir().unwrap();
        let mut f = ScenariosFile::load(tmp.path()).unwrap();
        f.put_entries(
            "billing",
            None,
            &[ScenarioEntry {
                endpoint: "GET /x".into(),
                id: "happy-path".into(),
                description: "ok".into(),
            }],
        )
        .unwrap();
        f.save().unwrap();
        let f2 = ScenariosFile::load(tmp.path()).unwrap();
        match f2.section("billing").unwrap().unwrap() {
            SlugSection::Flat(v) => assert_eq!(v.len(), 1),
            _ => panic!(),
        }
    }

    #[test]
    fn put_per_api_type_creates_bucket_mapping() {
        let tmp = tempdir().unwrap();
        let mut f = ScenariosFile::load(tmp.path()).unwrap();
        f.put_entries(
            "payment",
            Some("web-api"),
            &[ScenarioEntry {
                endpoint: "POST /w".into(),
                id: "a".into(),
                description: "ok".into(),
            }],
        )
        .unwrap();
        f.save().unwrap();
        let f2 = ScenariosFile::load(tmp.path()).unwrap();
        match f2.section("payment").unwrap().unwrap() {
            SlugSection::PerApiType(buckets) => {
                assert_eq!(buckets.len(), 1);
                assert_eq!(buckets[0].0, "web-api");
            }
            _ => panic!("expected per-api-type"),
        }
    }

    #[test]
    fn put_api_typed_into_existing_flat_with_entries_errors() {
        let tmp = tempdir().unwrap();
        write(
            &tmp.path().join("docs/test-scenarios.yaml"),
            r#"domains:
  billing:
    - endpoint: GET /x
      id: a
      description: ok
"#,
        );
        let mut f = ScenariosFile::load(tmp.path()).unwrap();
        let r = f.put_entries(
            "billing",
            Some("web-api"),
            &[ScenarioEntry {
                endpoint: "GET /y".into(),
                id: "b".into(),
                description: "ok".into(),
            }],
        );
        assert!(r.is_err(), "expected layout-switch refusal");
    }

    #[test]
    fn ensure_slug_seeded_idempotent() {
        let tmp = tempdir().unwrap();
        let mut f = ScenariosFile::load(tmp.path()).unwrap();
        f.ensure_slug_seeded("billing");
        f.ensure_slug_seeded("billing");
        f.save().unwrap();
        let f2 = ScenariosFile::load(tmp.path()).unwrap();
        match f2.section("billing").unwrap().unwrap() {
            SlugSection::Flat(v) => assert!(v.is_empty()),
            _ => panic!(),
        }
    }

    #[test]
    fn ensure_slug_seeded_does_not_clobber_existing_entries() {
        let tmp = tempdir().unwrap();
        write(
            &tmp.path().join("docs/test-scenarios.yaml"),
            r#"domains:
  billing:
    - endpoint: GET /x
      id: a
      description: ok
"#,
        );
        let mut f = ScenariosFile::load(tmp.path()).unwrap();
        f.ensure_slug_seeded("billing");
        f.save().unwrap();
        let f2 = ScenariosFile::load(tmp.path()).unwrap();
        match f2.section("billing").unwrap().unwrap() {
            SlugSection::Flat(v) => assert_eq!(v.len(), 1),
            _ => panic!(),
        }
    }
}
