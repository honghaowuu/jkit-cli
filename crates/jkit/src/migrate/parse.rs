use anyhow::{Context, Result};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use tree_sitter::{Node, Parser, Tree};
use walkdir::WalkDir;

/// One parsed Java compilation unit: package, classes, and the source the tree
/// borrows into. Source is owned to keep node ranges valid.
pub struct JavaFile {
    pub path: PathBuf,
    pub source: String,
    pub package: Option<String>,
    pub tree: Tree,
}

/// One class-like declaration extracted from a `JavaFile`. We don't recurse
/// into nested classes — controllers, services, repositories at a project's
/// top level cover the migration use case.
#[derive(Debug, Clone)]
pub struct ClassDecl {
    pub fqn: String,
    pub simple_name: String,
    /// Class-level annotations.
    pub annotations: Vec<Annotation>,
    /// First paragraph of the class-level Javadoc block (`/** ... */`)
    /// immediately preceding the class declaration. `None` when absent or
    /// empty after stripping leading `*` markers.
    pub javadoc: Option<String>,
    pub fields: Vec<FieldDecl>,
    pub methods: Vec<MethodDecl>,
}

#[derive(Debug, Clone)]
pub struct FieldDecl {
    pub name: String,
    /// Raw source text of the type expression, e.g. `InvoiceService` or
    /// `List<Invoice>`. We don't strip generics — the simple-name resolver
    /// peels them off when needed.
    pub type_text: String,
    /// Field-level annotations.
    pub annotations: Vec<Annotation>,
}

/// An annotation captured by the parser. `name` is the simple form
/// (`@OneToMany` → `"OneToMany"`); `args_raw` holds the raw text inside the
/// parentheses for `@Foo(bar = "baz")`-style annotations, or `None` for marker
/// annotations like `@Entity`.
#[derive(Debug, Clone)]
pub struct Annotation {
    pub name: String,
    pub args_raw: Option<String>,
}

impl Annotation {
    pub fn name(&self) -> &str {
        &self.name
    }
}

#[derive(Debug, Clone)]
pub struct MethodDecl {
    pub name: String,
    pub parameters: Vec<(String, String)>,
    pub invocations: Vec<Invocation>,
}

#[derive(Debug, Clone)]
pub struct Invocation {
    /// Receiver expression source text — `invoiceService`, `this.svc`,
    /// `Optional.of(...)`, etc. `None` for calls without a receiver
    /// (`super(...)`, `this(...)`, calls to inherited methods of the same
    /// class). The resolver keys on this verbatim and uses simple heuristics.
    pub receiver: Option<String>,
    pub method: String,
    pub arg_count: usize,
}

pub fn parse_file(path: &Path) -> Result<JavaFile> {
    let source = fs::read_to_string(path)
        .with_context(|| format!("reading {}", path.display()))?;
    let mut parser = Parser::new();
    let language: tree_sitter::Language = tree_sitter_java::LANGUAGE.into();
    parser
        .set_language(&language)
        .context("loading tree-sitter-java grammar")?;
    let tree = parser
        .parse(&source, None)
        .with_context(|| format!("tree-sitter parse failed: {}", path.display()))?;
    let package = extract_package(tree.root_node(), source.as_bytes());
    Ok(JavaFile {
        path: path.to_path_buf(),
        source,
        package,
        tree,
    })
}

pub fn parse_dir(src_root: &Path) -> Result<Vec<JavaFile>> {
    let mut out = Vec::new();
    for entry in WalkDir::new(src_root).into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("java") {
            continue;
        }
        match parse_file(path) {
            Ok(f) => out.push(f),
            Err(e) => {
                eprintln!("warning: skipping {}: {e}", path.display());
            }
        }
    }
    Ok(out)
}

pub fn extract_classes(file: &JavaFile) -> Vec<ClassDecl> {
    let bytes = file.source.as_bytes();
    let mut out = Vec::new();
    let root = file.tree.root_node();
    let mut cursor = root.walk();
    let children: Vec<Node> = root.children(&mut cursor).collect();
    for (i, child) in children.iter().enumerate() {
        if child.kind() == "class_declaration" || child.kind() == "interface_declaration" {
            // The Javadoc block (`/** ... */`) sits as a sibling immediately
            // before the class_declaration in the program-level child list.
            let preceding_doc = (0..i)
                .rev()
                .find_map(|j| {
                    let prev = children[j];
                    match prev.kind() {
                        "block_comment" | "line_comment" => Some(prev),
                        _ => None,
                    }
                })
                .filter(|n| n.kind() == "block_comment")
                .and_then(|n| node_text(n, bytes))
                .and_then(parse_javadoc_first_paragraph);
            if let Some(class) = extract_class(*child, bytes, file.package.as_deref(), preceding_doc) {
                out.push(class);
            }
        }
    }
    out
}

fn extract_class(
    node: Node,
    src: &[u8],
    package: Option<&str>,
    javadoc: Option<String>,
) -> Option<ClassDecl> {
    let simple_name = node
        .child_by_field_name("name")
        .and_then(|n| node_text(n, src).map(str::to_string))?;
    let fqn = match package {
        Some(p) if !p.is_empty() => format!("{p}.{simple_name}"),
        _ => simple_name.clone(),
    };

    // Class-level annotations are siblings of the name node under the
    // class_declaration. tree-sitter-java exposes them as `marker_annotation`
    // (`@Foo`) or `annotation` (`@Foo(...)`).
    let annotations = collect_annotations_at(node, src);

    let mut fields = Vec::new();
    let mut methods = Vec::new();
    if let Some(body) = node.child_by_field_name("body") {
        let mut cursor = body.walk();
        for child in body.children(&mut cursor) {
            match child.kind() {
                "field_declaration" => {
                    fields.extend(extract_fields(child, src));
                }
                "method_declaration" | "constructor_declaration" => {
                    if let Some(m) = extract_method(child, src) {
                        methods.push(m);
                    }
                }
                _ => {}
            }
        }
    }

    Some(ClassDecl {
        fqn,
        simple_name,
        annotations,
        javadoc,
        fields,
        methods,
    })
}

/// Walk the immediate children of `node` (typically a class_declaration or a
/// field_declaration's `modifiers` child) and collect annotations.
fn collect_annotations_at(node: Node, src: &[u8]) -> Vec<Annotation> {
    let mut out = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "marker_annotation" | "annotation" => {
                if let Some(a) = read_annotation(child, src) {
                    out.push(a);
                }
            }
            "modifiers" => {
                // field_declaration / method_declaration wrap annotations
                // inside a modifiers node.
                let mut sub = child.walk();
                for c in child.children(&mut sub) {
                    if matches!(c.kind(), "marker_annotation" | "annotation") {
                        if let Some(a) = read_annotation(c, src) {
                            out.push(a);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    out
}

fn read_annotation(node: Node, src: &[u8]) -> Option<Annotation> {
    let name = node
        .child_by_field_name("name")
        .and_then(|n| node_text(n, src))
        .map(simple_annotation_name)?;
    let args_raw = node.child_by_field_name("arguments").and_then(|args| {
        node_text(args, src).map(|s| {
            // The arguments node text includes the surrounding parens.
            let trimmed = s.trim();
            let inner = trimmed
                .strip_prefix('(')
                .and_then(|s| s.strip_suffix(')'))
                .unwrap_or(trimmed);
            inner.trim().to_string()
        })
    });
    Some(Annotation { name, args_raw })
}

fn simple_annotation_name(name: &str) -> String {
    name.rsplit('.').next().unwrap_or(name).to_string()
}

/// Strip Javadoc `/** ... */` markers and leading `*` per line, return the
/// first paragraph (text up to the first blank line or `@tag`). `None` when
/// the input isn't a Javadoc block or the result is empty.
pub fn parse_javadoc_first_paragraph(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if !raw.starts_with("/**") {
        return None;
    }
    let inner = raw.trim_start_matches("/**").trim_end_matches("*/");
    let mut lines: Vec<String> = Vec::new();
    for line in inner.lines() {
        let trimmed = line.trim().trim_start_matches('*').trim();
        if trimmed.starts_with('@') {
            break;
        }
        if trimmed.is_empty() {
            if !lines.is_empty() {
                break;
            }
            continue;
        }
        lines.push(trimmed.to_string());
    }
    if lines.is_empty() {
        None
    } else {
        Some(lines.join(" "))
    }
}

fn extract_fields(node: Node, src: &[u8]) -> Vec<FieldDecl> {
    let mut out = Vec::new();
    let type_text = node
        .child_by_field_name("type")
        .and_then(|n| node_text(n, src))
        .unwrap_or("")
        .to_string();
    let annotations = collect_annotations_at(node, src);
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "variable_declarator" {
            if let Some(name) = child
                .child_by_field_name("name")
                .and_then(|n| node_text(n, src))
            {
                out.push(FieldDecl {
                    name: name.to_string(),
                    type_text: type_text.clone(),
                    annotations: annotations.clone(),
                });
            }
        }
    }
    out
}

fn extract_method(node: Node, src: &[u8]) -> Option<MethodDecl> {
    let name = node
        .child_by_field_name("name")
        .and_then(|n| node_text(n, src).map(str::to_string))?;
    let parameters = node
        .child_by_field_name("parameters")
        .map(|p| extract_params(p, src))
        .unwrap_or_default();
    let invocations = node
        .child_by_field_name("body")
        .map(|b| collect_invocations(b, src))
        .unwrap_or_default();
    Some(MethodDecl {
        name,
        parameters,
        invocations,
    })
}

fn extract_params(node: Node, src: &[u8]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "formal_parameter" {
            let type_text = child
                .child_by_field_name("type")
                .and_then(|n| node_text(n, src))
                .unwrap_or("")
                .to_string();
            let name = child
                .child_by_field_name("name")
                .and_then(|n| node_text(n, src))
                .unwrap_or("")
                .to_string();
            out.push((name, type_text));
        }
    }
    out
}

fn collect_invocations(node: Node, src: &[u8]) -> Vec<Invocation> {
    let mut out = Vec::new();
    walk_invocations(node, src, &mut out);
    out
}

fn walk_invocations(node: Node, src: &[u8], out: &mut Vec<Invocation>) {
    if node.kind() == "method_invocation" {
        let method = node
            .child_by_field_name("name")
            .and_then(|n| node_text(n, src))
            .unwrap_or("")
            .to_string();
        let receiver = node
            .child_by_field_name("object")
            .and_then(|o| node_text(o, src))
            .map(|s| s.to_string());
        let arg_count = node
            .child_by_field_name("arguments")
            .map(|a| {
                let mut c = a.walk();
                a.children(&mut c)
                    .filter(|ch| !matches!(ch.kind(), "(" | ")" | ","))
                    .count()
            })
            .unwrap_or(0);
        if !method.is_empty() {
            out.push(Invocation {
                receiver,
                method,
                arg_count,
            });
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk_invocations(child, src, out);
    }
}

fn extract_package(root: Node, src: &[u8]) -> Option<String> {
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        if child.kind() == "package_declaration" {
            // The package name is the first scoped_identifier or identifier child.
            let mut sub = child.walk();
            for c in child.children(&mut sub) {
                if c.kind() == "scoped_identifier" || c.kind() == "identifier" {
                    return node_text(c, src).map(|s| s.to_string());
                }
            }
        }
    }
    None
}

fn node_text<'a>(n: Node, src: &'a [u8]) -> Option<&'a str> {
    n.utf8_text(src).ok()
}

/// Build a `simple_name → fqn` lookup from a parsed project. When a simple
/// name resolves to multiple FQNs, the lookup omits it (ambiguous — caller
/// surfaces as unresolved).
pub fn build_class_index(files: &[JavaFile]) -> HashMap<String, String> {
    let mut by_simple: HashMap<String, Vec<String>> = HashMap::new();
    for f in files {
        for class in extract_classes(f) {
            by_simple
                .entry(class.simple_name.clone())
                .or_default()
                .push(class.fqn.clone());
        }
    }
    by_simple
        .into_iter()
        .filter_map(|(k, v)| if v.len() == 1 { Some((k, v.into_iter().next().unwrap())) } else { None })
        .collect()
}

/// Strip generic type arguments — `List<Invoice>` → `List`, `Map<K, V>` → `Map`.
pub fn strip_generics(t: &str) -> &str {
    match t.find('<') {
        Some(i) => t[..i].trim(),
        None => t.trim(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_java(dir: &Path, name: &str, contents: &str) -> PathBuf {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn parses_class_with_fields_and_methods() {
        let dir = TempDir::new().unwrap();
        let path = write_java(
            dir.path(),
            "InvoiceController.java",
            r#"
package com.example.billing;

import org.springframework.web.bind.annotation.*;

@RestController
public class InvoiceController {
    private final InvoiceService invoiceService;

    public InvoiceController(InvoiceService svc) { this.invoiceService = svc; }

    @GetMapping("/invoices/{id}")
    public Invoice getById(@PathVariable String id) {
        return invoiceService.findById(id);
    }
}
"#,
        );
        let f = parse_file(&path).unwrap();
        assert_eq!(f.package.as_deref(), Some("com.example.billing"));
        let classes = extract_classes(&f);
        assert_eq!(classes.len(), 1);
        let c = &classes[0];
        assert_eq!(c.fqn, "com.example.billing.InvoiceController");
        assert_eq!(c.simple_name, "InvoiceController");
        assert_eq!(c.fields.len(), 1);
        assert_eq!(c.fields[0].name, "invoiceService");
        assert_eq!(c.fields[0].type_text, "InvoiceService");
        // Constructor + getById = 2.
        assert_eq!(c.methods.len(), 2);
        let getter = c.methods.iter().find(|m| m.name == "getById").unwrap();
        assert_eq!(getter.parameters, vec![("id".to_string(), "String".to_string())]);
        assert_eq!(getter.invocations.len(), 1);
        let inv = &getter.invocations[0];
        assert_eq!(inv.method, "findById");
        assert_eq!(inv.receiver.as_deref(), Some("invoiceService"));
        assert_eq!(inv.arg_count, 1);
    }

    #[test]
    fn class_index_filters_ambiguous() {
        let dir = TempDir::new().unwrap();
        write_java(
            dir.path(),
            "a/InvoiceService.java",
            "package a; public class InvoiceService {}",
        );
        write_java(
            dir.path(),
            "b/InvoiceService.java",
            "package b; public class InvoiceService {}",
        );
        write_java(
            dir.path(),
            "Unique.java",
            "package c; public class Unique {}",
        );
        let files = parse_dir(dir.path()).unwrap();
        let idx = build_class_index(&files);
        assert_eq!(idx.get("Unique"), Some(&"c.Unique".to_string()));
        assert!(!idx.contains_key("InvoiceService"));
    }

    #[test]
    fn extracts_class_and_field_annotations() {
        let dir = TempDir::new().unwrap();
        let path = write_java(
            dir.path(),
            "Invoice.java",
            r#"
package com.example;

import javax.persistence.*;
import javax.validation.constraints.*;

@Entity
@Table(name = "invoices")
public class Invoice {
    @Id
    @GeneratedValue
    private Long id;

    @NotNull
    @Column(nullable = false)
    private String customerId;
}
"#,
        );
        let f = parse_file(&path).unwrap();
        let classes = extract_classes(&f);
        assert_eq!(classes.len(), 1);
        let c = &classes[0];
        assert!(c.annotations.iter().any(|a| a.name == "Entity"));
        let table = c.annotations.iter().find(|a| a.name == "Table").unwrap();
        assert_eq!(table.args_raw.as_deref(), Some("name = \"invoices\""));
        let id = c.fields.iter().find(|f| f.name == "id").unwrap();
        assert!(id.annotations.iter().any(|a| a.name == "Id"));
        assert!(id.annotations.iter().any(|a| a.name == "GeneratedValue"));
        let cust = c.fields.iter().find(|f| f.name == "customerId").unwrap();
        assert!(cust.annotations.iter().any(|a| a.name == "NotNull"));
        let col = cust.annotations.iter().find(|a| a.name == "Column").unwrap();
        assert_eq!(col.args_raw.as_deref(), Some("nullable = false"));
    }

    #[test]
    fn captures_class_javadoc_first_paragraph() {
        let dir = TempDir::new().unwrap();
        let path = write_java(
            dir.path(),
            "Invoice.java",
            r#"
package com.example;

/**
 * Invoice issued to a customer for billable services.
 *
 * Has line items and a status. Settles via payments.
 *
 * @author someone
 */
public class Invoice {
    private Long id;
}
"#,
        );
        let f = parse_file(&path).unwrap();
        let classes = extract_classes(&f);
        assert_eq!(classes.len(), 1);
        assert_eq!(
            classes[0].javadoc.as_deref(),
            Some("Invoice issued to a customer for billable services.")
        );
    }

    #[test]
    fn class_javadoc_none_when_absent() {
        let dir = TempDir::new().unwrap();
        let path = write_java(
            dir.path(),
            "Plain.java",
            "package com.example; public class Plain {}",
        );
        let f = parse_file(&path).unwrap();
        let classes = extract_classes(&f);
        assert_eq!(classes[0].javadoc, None);
    }

    #[test]
    fn strips_generics() {
        assert_eq!(strip_generics("List<Invoice>"), "List");
        assert_eq!(strip_generics("Map<K, V>"), "Map");
        assert_eq!(strip_generics("Invoice"), "Invoice");
        assert_eq!(strip_generics("  Optional<Invoice>  "), "Optional");
    }
}
