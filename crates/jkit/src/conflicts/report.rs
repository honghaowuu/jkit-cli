use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::Path;

use crate::migrate::parse::{self, ClassDecl, FieldDecl};
use crate::util::print_json;

#[derive(Serialize)]
struct ConflictsReport {
    domains: Vec<DomainConflicts>,
    warnings: Vec<String>,
}

#[derive(Serialize)]
struct DomainConflicts {
    name: String,
    conflicts: Vec<Conflict>,
}

#[derive(Serialize, Clone)]
struct Conflict {
    category: &'static str,
    location: String,
    field: String,
    dto_class: String,
    dto_type: String,
    entity_class: String,
    entity_type: String,
    description: String,
}

#[derive(Deserialize)]
struct DomainMap {
    #[allow(dead_code)]
    schema_version: Option<u32>,
    domains: Vec<DomainMapping>,
}

#[derive(Deserialize)]
struct DomainMapping {
    slug: String,
    controllers: Vec<String>,
}

const ENTITY_ANNOTATIONS: &[&str] = &["Entity", "TableName"];

/// Suffix patterns that mark a class as a candidate DTO for an entity. The
/// matcher strips one of these (and optionally a prefix from `DTO_PREFIXES`)
/// from the class name; if the remainder equals the entity simple name, it's
/// a match. Order matters here only for prefix-iteration; the actual logic
/// tries them all.
const DTO_SUFFIXES: &[&str] = &[
    "RequestDto",
    "ResponseDto",
    "CreateRequest",
    "UpdateRequest",
    "PatchRequest",
    "CreateDto",
    "UpdateDto",
    "PatchDto",
    "Dto",
    "DTO",
    "Request",
    "Response",
    "Form",
    "Params",
    "Param",
    "VO",
    "Vo",
    "BO",
    "Bo",
];
const DTO_PREFIXES: &[&str] = &["Create", "Update", "Patch", "New", "List", "Get"];

const VALIDATION_NONNULL: &[&str] = &["NotNull", "NotBlank", "NotEmpty"];

pub fn run(
    domain_label: Option<&str>,
    src_root: &Path,
    pom: &Path,
    domain_map_path: Option<&Path>,
) -> Result<()> {
    let _ = pom; // reserved for parity with siblings; future filters may need it

    let files = parse::parse_dir(src_root)?;
    let all_classes: Vec<ClassDecl> = files
        .iter()
        .flat_map(|f| parse::extract_classes(f))
        .collect();

    let entities: Vec<&ClassDecl> = all_classes
        .iter()
        .filter(|c| {
            c.annotations
                .iter()
                .any(|a| ENTITY_ANNOTATIONS.contains(&a.as_str()))
        })
        .collect();

    let mut all_conflicts: Vec<Conflict> = Vec::new();
    for entity in &entities {
        let candidates = find_dto_candidates(&entity.simple_name, &all_classes);
        for dto in candidates {
            let conflicts = compare_dto_to_entity(dto, entity);
            all_conflicts.extend(conflicts);
        }
    }

    let mut warnings = Vec::new();

    let domains: Vec<DomainConflicts> = if let Some(p) = domain_map_path {
        match group_by_domain_map(&all_conflicts, &all_classes, p) {
            Ok(grouped) => grouped,
            Err(e) => {
                warnings.push(format!("domain-map grouping failed: {e}"));
                vec![DomainConflicts {
                    name: domain_label.unwrap_or("(all)").to_string(),
                    conflicts: all_conflicts,
                }]
            }
        }
    } else if all_conflicts.is_empty() {
        Vec::new()
    } else {
        vec![DomainConflicts {
            name: domain_label.unwrap_or("(all)").to_string(),
            conflicts: all_conflicts,
        }]
    };

    let report = ConflictsReport { domains, warnings };
    print_json(&report)
}

fn find_dto_candidates<'a>(
    entity_name: &str,
    all: &'a [ClassDecl],
) -> Vec<&'a ClassDecl> {
    let mut out = Vec::new();
    for c in all {
        if c.simple_name == entity_name {
            // Same-name class — could be the entity itself or a duplicate. Skip.
            continue;
        }
        if c.annotations
            .iter()
            .any(|a| ENTITY_ANNOTATIONS.contains(&a.as_str()))
        {
            // Don't pair entities with each other.
            continue;
        }
        if name_matches_entity(&c.simple_name, entity_name) {
            out.push(c);
        }
    }
    out
}

fn name_matches_entity(class_name: &str, entity_name: &str) -> bool {
    // Try suffix-only first.
    for suffix in DTO_SUFFIXES {
        if let Some(stripped) = class_name.strip_suffix(suffix) {
            if stripped == entity_name {
                return true;
            }
            // Suffix + prefix combo.
            for prefix in DTO_PREFIXES {
                if let Some(p) = stripped.strip_prefix(prefix) {
                    if p == entity_name {
                        return true;
                    }
                }
            }
        }
    }
    // Prefix-only.
    for prefix in DTO_PREFIXES {
        if let Some(p) = class_name.strip_prefix(prefix) {
            if p == entity_name {
                return true;
            }
        }
    }
    false
}

fn compare_dto_to_entity(dto: &ClassDecl, entity: &ClassDecl) -> Vec<Conflict> {
    let mut out = Vec::new();
    let entity_fields: HashMap<&str, &FieldDecl> =
        entity.fields.iter().map(|f| (f.name.as_str(), f)).collect();

    for dto_f in &dto.fields {
        let Some(entity_f) = entity_fields.get(dto_f.name.as_str()) else {
            continue;
        };

        let dto_t = normalize_type(&dto_f.type_text);
        let ent_t = normalize_type(&entity_f.type_text);
        if dto_t != ent_t {
            out.push(Conflict {
                category: "DtoEntityType",
                location: format!("{}.{}", dto.simple_name, dto_f.name),
                field: dto_f.name.clone(),
                dto_class: dto.fqn.clone(),
                dto_type: dto_f.type_text.clone(),
                entity_class: entity.fqn.clone(),
                entity_type: entity_f.type_text.clone(),
                description: format!(
                    "DTO field '{}' is `{}`; entity field is `{}`",
                    dto_f.name, dto_f.type_text, entity_f.type_text
                ),
            });
        }

        if entity_f
            .annotations
            .iter()
            .any(|a| a.as_str() == "NotNull")
            && !dto_f
                .annotations
                .iter()
                .any(|a| VALIDATION_NONNULL.contains(&a.as_str()))
        {
            out.push(Conflict {
                category: "DtoEntityValidation",
                location: format!("{}.{}", dto.simple_name, dto_f.name),
                field: dto_f.name.clone(),
                dto_class: dto.fqn.clone(),
                dto_type: dto_f.type_text.clone(),
                entity_class: entity.fqn.clone(),
                entity_type: entity_f.type_text.clone(),
                description: format!(
                    "Entity has `@NotNull` on `{}` but DTO has no `@NotNull`/`@NotBlank`/`@NotEmpty`",
                    dto_f.name
                ),
            });
        }
    }
    out
}

fn normalize_type(t: &str) -> String {
    let trimmed = t.trim();
    let stripped = trimmed.strip_prefix("java.lang.").unwrap_or(trimmed);
    let normalized = match stripped {
        "int" => "Integer",
        "long" => "Long",
        "double" => "Double",
        "float" => "Float",
        "boolean" => "Boolean",
        "byte" => "Byte",
        "char" => "Character",
        "short" => "Short",
        _ => stripped,
    };
    normalized.to_string()
}

/// Group flat conflict list by the resolved domain a controller belongs to,
/// using the package of the entity class as the join key. Entities in a
/// package matching one of a domain's controllers (any-prefix match) are
/// attributed to that domain. Conflicts on entities not attributable to any
/// domain are bucketed under "(unassigned)".
fn group_by_domain_map(
    conflicts: &[Conflict],
    all_classes: &[ClassDecl],
    map_path: &Path,
) -> Result<Vec<DomainConflicts>> {
    let raw =
        fs::read_to_string(map_path).with_context(|| format!("reading {}", map_path.display()))?;
    let parsed: DomainMap = serde_json::from_str(&raw)
        .with_context(|| format!("parsing {} as domain map", map_path.display()))?;

    // controller FQN -> slug
    let mut ctrl_to_slug: HashMap<String, String> = HashMap::new();
    for d in &parsed.domains {
        for c in &d.controllers {
            ctrl_to_slug.insert(c.clone(), d.slug.clone());
        }
    }

    // Build slug -> set of package prefixes (from controllers under that slug)
    let mut slug_packages: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for c in all_classes {
        if let Some(slug) = ctrl_to_slug.get(&c.fqn) {
            let pkg = c.fqn.rsplitn(2, '.').nth(1).unwrap_or("").to_string();
            slug_packages.entry(slug.clone()).or_default().push(pkg);
        }
    }

    // entity FQN -> domain slug (best match: entity package == or starts with controller package)
    let entity_pkgs: HashMap<&str, String> = all_classes
        .iter()
        .filter(|c| {
            c.annotations
                .iter()
                .any(|a| ENTITY_ANNOTATIONS.contains(&a.as_str()))
        })
        .map(|c| {
            let pkg = c.fqn.rsplitn(2, '.').nth(1).unwrap_or("").to_string();
            (c.fqn.as_str(), pkg)
        })
        .collect();

    let mut entity_to_slug: HashMap<&str, String> = HashMap::new();
    for (entity_fqn, entity_pkg) in &entity_pkgs {
        let mut best: Option<(usize, &String)> = None;
        for (slug, pkgs) in &slug_packages {
            for ctrl_pkg in pkgs {
                let common = common_prefix_len(entity_pkg, ctrl_pkg);
                if common > 0 && best.map(|(b, _)| common > b).unwrap_or(true) {
                    best = Some((common, slug));
                }
            }
        }
        if let Some((_, slug)) = best {
            entity_to_slug.insert(entity_fqn, slug.clone());
        }
    }

    let mut by_slug: BTreeMap<String, Vec<Conflict>> = BTreeMap::new();
    for c in conflicts {
        let slug = entity_to_slug
            .get(c.entity_class.as_str())
            .cloned()
            .unwrap_or_else(|| "(unassigned)".to_string());
        by_slug.entry(slug).or_default().push(c.clone());
    }

    Ok(by_slug
        .into_iter()
        .map(|(name, conflicts)| DomainConflicts { name, conflicts })
        .collect())
}

fn common_prefix_len(a: &str, b: &str) -> usize {
    let mut count = 0;
    let mut ai = a.split('.');
    let mut bi = b.split('.');
    loop {
        match (ai.next(), bi.next()) {
            (Some(x), Some(y)) if x == y && !x.is_empty() => count += 1,
            _ => break,
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_dto_suffix_patterns() {
        assert!(name_matches_entity("InvoiceDto", "Invoice"));
        assert!(name_matches_entity("InvoiceRequest", "Invoice"));
        assert!(name_matches_entity("InvoiceCreateRequest", "Invoice"));
        assert!(name_matches_entity("CreateInvoiceRequest", "Invoice"));
        assert!(name_matches_entity("CreateInvoiceDto", "Invoice"));
        assert!(!name_matches_entity("Invoice", "Invoice"));
        assert!(!name_matches_entity("InvoiceController", "Invoice"));
        assert!(!name_matches_entity("Customer", "Invoice"));
    }

    #[test]
    fn normalizes_primitive_box_pairs() {
        assert_eq!(normalize_type("int"), "Integer");
        assert_eq!(normalize_type("Integer"), "Integer");
        assert_eq!(normalize_type("java.lang.String"), "String");
        assert_eq!(normalize_type("List<String>"), "List<String>");
    }

    #[test]
    fn common_prefix_counts_dotted_segments() {
        assert_eq!(common_prefix_len("a.b.c", "a.b.d"), 2);
        assert_eq!(common_prefix_len("a.b.c", "a.b.c"), 3);
        assert_eq!(common_prefix_len("a", "b"), 0);
        assert_eq!(common_prefix_len("", ""), 0);
    }
}
