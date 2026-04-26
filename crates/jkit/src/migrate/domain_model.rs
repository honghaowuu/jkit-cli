//! Best-effort `domain-model.md` drafting from `@Entity` / `@TableName` classes.
//!
//! Migrate-project Step 5.6 used to delegate this to Claude reading entity
//! source files. That worked but burned tokens and risked paraphrasing
//! annotations the source didn't have. This module does the same job
//! deterministically by walking the tree-sitter parse already produced for
//! `--use-call-graph`, then emits Markdown matching the template that the
//! migrate-project skill's `reference/domain-model.md` documents.

use std::collections::BTreeSet;

use crate::contract::service_meta::Controller;
use crate::migrate::parse::{Annotation, ClassDecl};

/// Annotations that mark a JPA entity or MyBatis table. Mirrors
/// `conflicts::report::ENTITY_ANNOTATIONS` — kept independent so each module
/// stays self-contained, but should change in lockstep.
pub const ENTITY_ANNOTATIONS: &[&str] = &["Entity", "TableName"];

/// Annotation names treated as relationship markers when listing field
/// relationships. Args (mappedBy / cascade / targetEntity) come from
/// `Annotation.args_raw`.
const RELATIONSHIP_ANNOTATIONS: &[&str] = &["OneToMany", "ManyToOne", "ManyToMany", "OneToOne"];

/// Annotations to surface in the field constraints column. Order is the
/// rendering order; everything not in this list is filtered out.
const RENDERED_FIELD_ANNOTATIONS: &[&str] = &[
    "Id",
    "GeneratedValue",
    "Column",
    "Enumerated",
    "Lob",
    "Temporal",
    "Version",
    "NotNull",
    "NotBlank",
    "NotEmpty",
    "Size",
    "Min",
    "Max",
    "Email",
    "Pattern",
    "Past",
    "Future",
];

/// Result of selecting which entities belong to a slug. `selected` is the
/// list to render; `excluded_simple_names` are entities that exist in source
/// but didn't match any inclusion rule for this slug — surfaced as a TODO at
/// the end of the rendered file so the human can pull missing entities back
/// in.
pub struct Selection<'a> {
    pub selected: Vec<&'a ClassDecl>,
    pub excluded_simple_names: Vec<String>,
}

/// Pick which entities to draft for a given slug.
///
/// Heuristic, in order:
/// 1. Entity package equals or is a sub-package of any controller's package
///    under this slug → include.
/// 2. Entity simple name appears as a whole word in the rendered impl-logic
///    text → include.
/// 3. Small-project fallback: if total entity count ≤ 10 and we matched
///    nothing for this slug, include all entities (warning surfaced at the
///    call site).
/// 4. Else exclude; the simple name lands in `excluded_simple_names`.
pub fn select_entities_for_slug<'a>(
    controllers_in_slug: &[&Controller],
    all_entities: &'a [&'a ClassDecl],
    impl_logic_md: Option<&str>,
) -> Selection<'a> {
    let controller_pkgs: Vec<&str> = controllers_in_slug
        .iter()
        .filter_map(|c| package_of(&c.class))
        .collect();

    let impl_text = impl_logic_md.unwrap_or("");

    let mut selected: Vec<&ClassDecl> = Vec::new();
    let mut excluded: Vec<String> = Vec::new();

    for entity in all_entities {
        let pkg = package_of(&entity.fqn).unwrap_or("");
        let pkg_match = controller_pkgs.iter().any(|cpkg| pkg_includes(cpkg, pkg));
        let name_match = !impl_text.is_empty() && contains_whole_word(impl_text, &entity.simple_name);
        if pkg_match || name_match {
            selected.push(*entity);
        } else {
            excluded.push(entity.simple_name.clone());
        }
    }

    if selected.is_empty() && all_entities.len() <= 10 && !controllers_in_slug.is_empty() {
        // Fallback: small project, no signal. Include everything; the human
        // will trim. Move excluded names back into selected so we don't
        // double-list.
        selected = all_entities.iter().copied().collect();
        excluded.clear();
    }

    Selection {
        selected,
        excluded_simple_names: excluded,
    }
}

fn package_of(fqn: &str) -> Option<&str> {
    fqn.rsplit_once('.').map(|(pkg, _)| pkg)
}

/// Returns true when `entity_pkg` is `controller_pkg` itself or a sub-package
/// of it. We never go the other direction — an entity in a parent package of
/// the controller is not domain-attributed by package alone.
fn pkg_includes(controller_pkg: &str, entity_pkg: &str) -> bool {
    if controller_pkg.is_empty() || entity_pkg.is_empty() {
        return false;
    }
    if entity_pkg == controller_pkg {
        return true;
    }
    if let Some(rest) = entity_pkg.strip_prefix(controller_pkg) {
        return rest.starts_with('.');
    }
    false
}

fn contains_whole_word(haystack: &str, needle: &str) -> bool {
    let bytes = haystack.as_bytes();
    let n = needle.as_bytes();
    if n.is_empty() {
        return false;
    }
    let mut i = 0;
    while i + n.len() <= bytes.len() {
        if &bytes[i..i + n.len()] == n {
            let before_ok = i == 0 || !is_word_byte(bytes[i - 1]);
            let after_ok = i + n.len() == bytes.len() || !is_word_byte(bytes[i + n.len()]);
            if before_ok && after_ok {
                return true;
            }
        }
        i += 1;
    }
    false
}

fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Marker the renderer plants in the file. Re-runs treat files containing
/// this string as ours (overwrite); files lacking it are treated as
/// human-edited (skip).
pub const AUTO_DRAFTED_MARKER: &str = "Auto-drafted by `jkit migrate scaffold-docs --draft-domain-model`";

/// Returns `true` when the file contents look hand-edited away from our
/// scaffold (no marker present). Empty contents → `false` (we'll write).
pub fn looks_human_edited(existing: &str) -> bool {
    !existing.is_empty() && !existing.contains(AUTO_DRAFTED_MARKER)
}

pub fn render(slug: &str, selection: &Selection<'_>) -> String {
    let mut s = String::new();
    s.push_str(&format!("# {slug} domain model\n\n"));
    s.push_str(&format!(
        "<!-- {AUTO_DRAFTED_MARKER} from @Entity classes. Refine via /spec-delta — this is best-effort inference, not authoritative. -->\n\n"
    ));

    if selection.selected.is_empty() {
        s.push_str("## Entities\n\n");
        s.push_str("<!-- TODO: no @Entity classes attributed to this domain. Either add entities, or attribute existing ones via package layout / api-implement-logic.md mentions. -->\n\n");
    } else {
        s.push_str("## Entities\n\n");
        for entity in &selection.selected {
            render_entity(&mut s, entity);
        }
    }

    s.push_str("## Cross-entity invariants\n\n");
    s.push_str("<!-- TODO: business invariants that span entities (e.g., \"invoice total must equal sum of line items\"). spec-delta will refine this. -->\n");

    if !selection.excluded_simple_names.is_empty() {
        s.push_str("\n## Excluded entities (review)\n\n");
        s.push_str("<!-- These @Entity classes exist in source but didn't match this domain by package proximity or impl-logic mention. Pull any back in by adding their package to this domain's controllers, or delete this section. -->\n\n");
        for name in &selection.excluded_simple_names {
            s.push_str(&format!("- `{name}` — TODO: include or ignore\n"));
        }
        s.push('\n');
    }

    s
}

fn render_entity(s: &mut String, entity: &ClassDecl) {
    s.push_str(&format!("### {} (`{}`)\n\n", entity.simple_name, entity.fqn));
    match entity.javadoc.as_deref() {
        Some(text) if !text.is_empty() => {
            s.push_str(text);
            s.push_str("\n\n");
        }
        _ => s.push_str("<!-- TODO: describe purpose -->\n\n"),
    }

    let non_relationship_fields: Vec<_> = entity
        .fields
        .iter()
        .filter(|f| !is_relationship_field(&f.annotations))
        .collect();

    if !non_relationship_fields.is_empty() {
        s.push_str("| Field | Type | Constraints |\n");
        s.push_str("|---|---|---|\n");
        for f in &non_relationship_fields {
            let rendered = render_field_annotations(&f.annotations);
            let constraints = if rendered.is_empty() {
                "—".to_string()
            } else {
                rendered
            };
            s.push_str(&format!(
                "| `{}` | `{}` | {} |\n",
                f.name, f.type_text, constraints
            ));
        }
        s.push('\n');
    }

    let relationships: Vec<_> = entity
        .fields
        .iter()
        .filter(|f| is_relationship_field(&f.annotations))
        .collect();

    if !relationships.is_empty() {
        s.push_str("**Relationships:**\n\n");
        for f in &relationships {
            let rel = f
                .annotations
                .iter()
                .find(|a| RELATIONSHIP_ANNOTATIONS.contains(&a.name.as_str()))
                .unwrap();
            let rel_args = match &rel.args_raw {
                Some(args) if !args.is_empty() => format!("({args})"),
                _ => String::new(),
            };
            s.push_str(&format!(
                "- `{}` → `{}` (`@{}{rel_args}`) — TODO: invariants\n",
                f.name, f.type_text, rel.name
            ));
        }
        s.push('\n');
    }
}

fn is_relationship_field(anns: &[Annotation]) -> bool {
    anns.iter()
        .any(|a| RELATIONSHIP_ANNOTATIONS.contains(&a.name.as_str()))
}

fn render_field_annotations(anns: &[Annotation]) -> String {
    // Collect by name to preserve order from RENDERED_FIELD_ANNOTATIONS while
    // dropping unknown / relationship markers.
    let by_name: BTreeSet<&str> = anns.iter().map(|a| a.name.as_str()).collect();
    let mut parts: Vec<String> = Vec::new();
    for &kind in RENDERED_FIELD_ANNOTATIONS {
        if by_name.contains(kind) {
            let ann = anns.iter().find(|a| a.name == kind).unwrap();
            match &ann.args_raw {
                Some(args) if !args.is_empty() => parts.push(format!("`@{kind}({args})`")),
                _ => parts.push(format!("`@{kind}`")),
            }
        }
    }
    parts.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::service_meta::Controller;
    use crate::domain_layout::ApiType;
    use crate::migrate::parse::{Annotation, FieldDecl};

    fn entity(fqn: &str, javadoc: Option<&str>, fields: Vec<FieldDecl>) -> ClassDecl {
        ClassDecl {
            fqn: fqn.to_string(),
            simple_name: fqn.rsplit('.').next().unwrap().to_string(),
            annotations: vec![Annotation {
                name: "Entity".to_string(),
                args_raw: None,
            }],
            javadoc: javadoc.map(String::from),
            fields,
            methods: vec![],
        }
    }

    fn field(name: &str, ty: &str, anns: Vec<(&str, Option<&str>)>) -> FieldDecl {
        FieldDecl {
            name: name.to_string(),
            type_text: ty.to_string(),
            annotations: anns
                .into_iter()
                .map(|(n, a)| Annotation {
                    name: n.to_string(),
                    args_raw: a.map(String::from),
                })
                .collect(),
        }
    }

    fn controller(class: &str) -> Controller {
        Controller {
            class: class.to_string(),
            file: String::new(),
            domain_slug: "x".to_string(),
            api_type: ApiType::WebApi,
            methods: vec![],
        }
    }

    #[test]
    fn package_match_picks_entities_in_or_under_controller_pkg() {
        let inv = entity("com.example.invoice.Invoice", None, vec![]);
        let line = entity("com.example.invoice.line.InvoiceLine", None, vec![]);
        let pay = entity("com.example.payment.Payment", None, vec![]);
        let entities = vec![&inv, &line, &pay];
        let ctrl = controller("com.example.invoice.InvoiceController");
        let ctrls = vec![&ctrl];
        let sel = select_entities_for_slug(&ctrls, &entities, None);
        let names: Vec<&str> = sel.selected.iter().map(|c| c.simple_name.as_str()).collect();
        assert!(names.contains(&"Invoice"));
        assert!(names.contains(&"InvoiceLine"));
        assert!(!names.contains(&"Payment"));
        assert_eq!(sel.excluded_simple_names, vec!["Payment".to_string()]);
    }

    #[test]
    fn name_mention_in_impl_logic_pulls_entity_in() {
        let aud = entity("com.other.audit.Audit", None, vec![]);
        let entities = vec![&aud];
        let ctrl = controller("com.example.invoice.InvoiceController");
        let ctrls = vec![&ctrl];
        let impl_md = "## InvoiceController#create — calls auditService.record(Audit ...)";
        let sel = select_entities_for_slug(&ctrls, &entities, Some(impl_md));
        assert_eq!(sel.selected.len(), 1);
        assert_eq!(sel.selected[0].simple_name, "Audit");
        assert!(sel.excluded_simple_names.is_empty());
    }

    #[test]
    fn small_project_fallback_includes_unattributed() {
        // Three entities, none in the controller's package; none mentioned.
        // ≤10 total → fallback fires.
        let a = entity("com.x.A", None, vec![]);
        let b = entity("com.y.B", None, vec![]);
        let c = entity("com.z.C", None, vec![]);
        let entities = vec![&a, &b, &c];
        let ctrl = controller("com.example.invoice.InvoiceController");
        let ctrls = vec![&ctrl];
        let sel = select_entities_for_slug(&ctrls, &entities, None);
        assert_eq!(sel.selected.len(), 3);
        assert!(sel.excluded_simple_names.is_empty());
    }

    #[test]
    fn renders_field_table_and_relationships() {
        let e = entity(
            "com.example.invoice.Invoice",
            Some("Invoice issued to a customer."),
            vec![
                field("id", "Long", vec![("Id", None), ("GeneratedValue", None)]),
                field(
                    "amount",
                    "BigDecimal",
                    vec![
                        ("NotNull", None),
                        ("Column", Some("precision = 19, scale = 2")),
                    ],
                ),
                field(
                    "customer",
                    "Customer",
                    vec![("ManyToOne", Some("fetch = FetchType.LAZY"))],
                ),
                field(
                    "lineItems",
                    "List<InvoiceLine>",
                    vec![("OneToMany", Some("mappedBy = \"invoice\""))],
                ),
            ],
        );
        let sel = Selection {
            selected: vec![&e],
            excluded_simple_names: vec![],
        };
        let out = render("invoice", &sel);
        assert!(out.contains("# invoice domain model"));
        assert!(out.contains(AUTO_DRAFTED_MARKER));
        assert!(out.contains("Invoice issued to a customer."));
        assert!(out.contains("| `id` | `Long` | `@Id`, `@GeneratedValue` |"));
        assert!(out.contains("`@NotNull`"));
        assert!(out.contains("`@Column(precision = 19, scale = 2)`"));
        // Relationships listed under their own heading, not in the field table.
        assert!(out.contains("**Relationships:**"));
        assert!(out.contains("`customer` → `Customer` (`@ManyToOne(fetch = FetchType.LAZY)`)"));
        assert!(out.contains("`lineItems` → `List<InvoiceLine>` (`@OneToMany(mappedBy = \"invoice\")`)"));
        assert!(!out.contains("| `customer`"));
        assert!(out.contains("## Cross-entity invariants"));
    }

    #[test]
    fn renders_excluded_section_when_present() {
        let e = entity("com.example.invoice.Invoice", None, vec![]);
        let sel = Selection {
            selected: vec![&e],
            excluded_simple_names: vec!["Audit".to_string(), "Setting".to_string()],
        };
        let out = render("invoice", &sel);
        assert!(out.contains("## Excluded entities (review)"));
        assert!(out.contains("- `Audit`"));
        assert!(out.contains("- `Setting`"));
    }

    #[test]
    fn empty_selection_writes_todo_section() {
        let sel = Selection {
            selected: vec![],
            excluded_simple_names: vec![],
        };
        let out = render("orphan", &sel);
        assert!(out.contains("no @Entity classes attributed to this domain"));
    }

    #[test]
    fn human_edited_detection() {
        assert!(!looks_human_edited(""));
        assert!(!looks_human_edited(&format!("# x\n<!-- {AUTO_DRAFTED_MARKER} -->\n")));
        assert!(looks_human_edited("# x\n\nhand-written content"));
    }

    #[test]
    fn pkg_includes_handles_subpackages() {
        assert!(pkg_includes("com.example.invoice", "com.example.invoice"));
        assert!(pkg_includes("com.example.invoice", "com.example.invoice.line"));
        assert!(!pkg_includes("com.example.invoice", "com.example.invoiceextra"));
        assert!(!pkg_includes("com.example.invoice", "com.example"));
        assert!(!pkg_includes("com.example.invoice", "com.other"));
    }

    #[test]
    fn whole_word_match_skips_substrings() {
        assert!(contains_whole_word("Invoice handled here", "Invoice"));
        assert!(contains_whole_word("create(Invoice)", "Invoice"));
        assert!(!contains_whole_word("InvoiceLine handled", "Invoice"));
        assert!(!contains_whole_word("BigInvoice handled", "Invoice"));
    }
}
