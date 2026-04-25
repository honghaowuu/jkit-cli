use anyhow::{anyhow, Result};
use quick_xml::events::Event;
use quick_xml::Reader;
use std::path::Path;

use crate::util::atomic_write;

/// Detect the dominant indent unit (spaces or tabs, and width) by scanning
/// leading whitespace of indented lines. Defaults to 4 spaces if undetectable.
pub fn detect_indent(text: &str) -> String {
    let mut tab_lines = 0usize;
    let mut space_widths: Vec<usize> = Vec::new();
    for line in text.lines() {
        let leading: String = line.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
        if leading.is_empty() {
            continue;
        }
        if leading.starts_with('\t') {
            tab_lines += 1;
            continue;
        }
        let n = leading.chars().take_while(|c| *c == ' ').count();
        if n > 0 {
            space_widths.push(n);
        }
    }
    if tab_lines > space_widths.len() {
        return "\t".to_string();
    }
    if space_widths.is_empty() {
        return "    ".to_string();
    }
    let g = space_widths
        .iter()
        .copied()
        .reduce(gcd)
        .unwrap_or(4);
    " ".repeat(g.max(1))
}

fn gcd(a: usize, b: usize) -> usize {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// A range in the source text representing the inner content of an element.
/// `inner_start` points just AFTER the opening tag's `>`; `inner_end` points to
/// the start of the closing `</tag>`. So `text[inner_start..inner_end]` is the
/// children-text we may want to scan or mutate.
#[derive(Debug, Clone)]
pub struct ElementSpan {
    pub inner_start: usize,
    pub inner_end: usize,
    /// Indentation of the closing `</tag>` line — useful for inserting a child
    /// at one level deeper than the parent.
    pub closing_indent: String,
}

/// Find the inner span of the first element matching the path (e.g. ["project","build","plugins"]).
/// Returns None if any path segment is missing. Self-closing tags (`<plugins/>`) are
/// reported with `inner_start == inner_end` at the position of the `/>`.
pub fn find_element_span(text: &str, path: &[&str]) -> Option<ElementSpan> {
    let mut reader = Reader::from_str(text);
    reader.config_mut().trim_text(false);
    let mut buf = Vec::new();
    let mut stack: Vec<String> = Vec::new();
    let mut target_open_pos: Option<usize> = None;

    loop {
        let pos_before = reader.buffer_position() as usize;
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let name = std::str::from_utf8(e.name().as_ref()).unwrap_or("").to_string();
                stack.push(name);
                if path_matches(&stack, path) && target_open_pos.is_none() {
                    // inner_start = current reader position (just past the '>')
                    target_open_pos = Some(reader.buffer_position() as usize);
                }
            }
            Ok(Event::End(e)) => {
                if path_matches(&stack, path) {
                    let inner_end = pos_before;
                    let closing_indent = leading_whitespace_at(text, inner_end);
                    return Some(ElementSpan {
                        inner_start: target_open_pos?,
                        inner_end,
                        closing_indent,
                    });
                }
                let _ = e;
                stack.pop();
            }
            Ok(Event::Empty(e)) => {
                let name = std::str::from_utf8(e.name().as_ref()).unwrap_or("").to_string();
                stack.push(name);
                if path_matches(&stack, path) {
                    let pos_after = reader.buffer_position() as usize;
                    // empty element occupies pos_before..pos_after; inner is empty.
                    let inner = pos_after.saturating_sub(2); // inside the "/>"
                    return Some(ElementSpan {
                        inner_start: inner,
                        inner_end: inner,
                        closing_indent: String::new(),
                    });
                }
                stack.pop();
            }
            Ok(Event::Eof) => return None,
            Err(_) => return None,
            _ => {}
        }
        buf.clear();
    }
}

fn path_matches(stack: &[String], path: &[&str]) -> bool {
    if stack.len() != path.len() {
        return false;
    }
    stack.iter().zip(path.iter()).all(|(a, b)| a == b)
}

/// Return the leading whitespace (spaces/tabs) of the line that contains `pos`.
fn leading_whitespace_at(text: &str, pos: usize) -> String {
    let pos = pos.min(text.len());
    let line_start = text[..pos].rfind('\n').map(|i| i + 1).unwrap_or(0);
    text[line_start..pos]
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect()
}

/// Search inside an inner-element span for a child element with a given attribute fingerprint.
/// `child_tag` is the element name to look for (e.g. "dependency", "plugin"); the function
/// reads each direct-child element and returns true if `predicate(text_of_child)` holds.
pub fn child_exists<F: Fn(&str) -> bool>(
    text: &str,
    span: &ElementSpan,
    child_tag: &str,
    predicate: F,
) -> bool {
    let inner = &text[span.inner_start..span.inner_end];
    let mut reader = Reader::from_str(inner);
    reader.config_mut().trim_text(false);
    let mut buf = Vec::new();
    let mut depth: usize = 0;
    let mut current_start_byte: Option<usize> = None;

    loop {
        let pos_before = reader.buffer_position() as usize;
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let name_owned = std::str::from_utf8(e.name().as_ref())
                    .unwrap_or("")
                    .to_string();
                let name = name_owned.as_str();
                if depth == 0 && name == child_tag {
                    current_start_byte = Some(pos_before);
                }
                depth += 1;
            }
            Ok(Event::End(_)) => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    if let Some(start) = current_start_byte.take() {
                        let end = reader.buffer_position() as usize;
                        let body = &inner[start..end];
                        if predicate(body) {
                            return true;
                        }
                    }
                }
            }
            Ok(Event::Empty(e)) => {
                let name_owned = std::str::from_utf8(e.name().as_ref())
                    .unwrap_or("")
                    .to_string();
                let name = name_owned.as_str();
                if depth == 0 && name == child_tag {
                    let end = reader.buffer_position() as usize;
                    let body = &inner[pos_before..end];
                    if predicate(body) {
                        return true;
                    }
                }
            }
            Ok(Event::Eof) => return false,
            Err(_) => return false,
            _ => {}
        }
        buf.clear();
    }
}

/// Extract the trimmed text content of a direct child element by tag name from
/// a fragment body (e.g. <dependency>...</dependency>).
pub fn child_text(body: &str, tag: &str) -> Option<String> {
    let open = format!("<{}>", tag);
    let close = format!("</{}>", tag);
    let i = body.find(&open)? + open.len();
    let j = body[i..].find(&close)? + i;
    Some(body[i..j].trim().to_string())
}

/// Insert a fragment (already indented to the child level — see `indent_fragment`)
/// just before the closing tag of `span`. Writes back to `pom_path` atomically.
pub fn insert_into_span(text: &str, span: &ElementSpan, fragment_text: &str) -> String {
    // Ensure there's a newline before the fragment, and the fragment ends with a newline so
    // the closing tag's indentation isn't disrupted.
    let mut out = String::with_capacity(text.len() + fragment_text.len() + 64);
    out.push_str(&text[..span.inner_end]);

    let needs_leading_newline = !out.ends_with('\n');
    if needs_leading_newline {
        out.push('\n');
    }
    // Strip leading newlines from the fragment to avoid double-blank-lines.
    let frag = fragment_text.trim_start_matches('\n');
    out.push_str(frag);
    if !out.ends_with('\n') {
        out.push('\n');
    }
    // Restore the closing-tag indent
    out.push_str(&span.closing_indent);
    out.push_str(&text[span.inner_end..]);
    out
}

/// Indent every line of `fragment` so the first line sits at `child_indent` and
/// subsequent indented lines preserve their relative offsets. Templates are stored
/// flush-left at zero indent with internal indents in multiples of 4 spaces.
pub fn indent_fragment(fragment: &str, child_indent: &str, indent_unit: &str) -> String {
    // Rewrite each leading-space run from "4-space units" into the target indent unit.
    let mut out = String::new();
    for (i, raw_line) in fragment.lines().enumerate() {
        if raw_line.trim().is_empty() {
            if i + 1 != fragment.lines().count() || fragment.ends_with('\n') {
                out.push('\n');
            }
            continue;
        }
        let leading_spaces = raw_line.chars().take_while(|c| *c == ' ').count();
        let depth = leading_spaces / 4;
        let body = &raw_line[leading_spaces..];
        out.push_str(child_indent);
        for _ in 0..depth {
            out.push_str(indent_unit);
        }
        out.push_str(body);
        out.push('\n');
    }
    out
}

/// Read pom file. Errors with anyhow context.
pub fn read_pom(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).map_err(|e| anyhow!("reading {}: {}", path.display(), e))
}

/// Write pom file atomically.
pub fn write_pom(path: &Path, text: &str) -> Result<()> {
    atomic_write(path, text.as_bytes())
}

/// Locate `<dependencies>` under `<project>`. If missing, returns None — callers
/// must decide whether to create one (apply mode) or report missing.
pub fn dependencies_span(text: &str) -> Option<ElementSpan> {
    find_element_span(text, &["project", "dependencies"])
}

/// Locate `<build><plugins>`. Same caveat as above.
pub fn plugins_span(text: &str) -> Option<ElementSpan> {
    find_element_span(text, &["project", "build", "plugins"])
}

/// Insert a `<dependencies>` element directly under `<project>` if missing,
/// returning the new pom text. Caller should re-locate the dependencies span
/// after this call.
pub fn ensure_dependencies(text: &str, indent_unit: &str) -> Result<String> {
    if dependencies_span(text).is_some() {
        return Ok(text.to_string());
    }
    let project = find_element_span(text, &["project"])
        .ok_or_else(|| anyhow!("pom.xml has no <project> root element"))?;
    let frag = format!("\n{i}<dependencies>\n{i}</dependencies>\n", i = indent_unit);
    Ok(insert_into_span(text, &project, &frag))
}

/// Same as `ensure_dependencies` but for `<build><plugins>`.
pub fn ensure_plugins(text: &str, indent_unit: &str) -> Result<String> {
    if plugins_span(text).is_some() {
        return Ok(text.to_string());
    }
    // First make sure <build> exists.
    let mut working = text.to_string();
    if find_element_span(&working, &["project", "build"]).is_none() {
        let project = find_element_span(&working, &["project"])
            .ok_or_else(|| anyhow!("pom.xml has no <project> root element"))?;
        let frag = format!(
            "\n{i}<build>\n{i}{i}<plugins>\n{i}{i}</plugins>\n{i}</build>\n",
            i = indent_unit
        );
        working = insert_into_span(&working, &project, &frag);
        return Ok(working);
    }
    let build = find_element_span(&working, &["project", "build"])
        .ok_or_else(|| anyhow!("pom.xml has <build> but it could not be located after parse"))?;
    let frag = format!("\n{i}<plugins>\n{i}</plugins>\n", i = indent_unit);
    Ok(insert_into_span(&working, &build, &frag))
}

#[cfg(test)]
mod tests {
    use super::*;

    const POM_BASIC: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<project xmlns="http://maven.apache.org/POM/4.0.0">
    <modelVersion>4.0.0</modelVersion>
    <groupId>com.example</groupId>
    <artifactId>demo</artifactId>
    <version>1.0.0</version>
    <dependencies>
        <dependency>
            <groupId>org.springframework.boot</groupId>
            <artifactId>spring-boot-starter-web</artifactId>
        </dependency>
    </dependencies>
</project>
"#;

    #[test]
    fn detect_indent_picks_4() {
        assert_eq!(detect_indent(POM_BASIC), "    ");
    }

    #[test]
    fn detect_indent_falls_back_to_4_when_undetectable() {
        assert_eq!(detect_indent("hello\nworld\n"), "    ");
    }

    #[test]
    fn detect_indent_picks_tab() {
        let s = "<a>\n\t<b/>\n\t<c>\n\t\t<d/>\n\t</c>\n</a>\n";
        assert_eq!(detect_indent(s), "\t");
    }

    #[test]
    fn finds_dependencies_span() {
        let span = dependencies_span(POM_BASIC).expect("dependencies present");
        assert!(POM_BASIC[span.inner_start..span.inner_end].contains("spring-boot-starter-web"));
    }

    #[test]
    fn child_exists_matches_dep() {
        let span = dependencies_span(POM_BASIC).unwrap();
        let found = child_exists(POM_BASIC, &span, "dependency", |body| {
            child_text(body, "groupId").as_deref() == Some("org.springframework.boot")
                && child_text(body, "artifactId").as_deref() == Some("spring-boot-starter-web")
        });
        assert!(found);
    }

    #[test]
    fn ensure_plugins_creates_build_and_plugins_when_missing() {
        let out = ensure_plugins(POM_BASIC, "    ").unwrap();
        assert!(plugins_span(&out).is_some());
    }

    #[test]
    fn insert_into_span_preserves_closing_indent() {
        let span = dependencies_span(POM_BASIC).unwrap();
        let frag = "        <dependency>\n            <groupId>x</groupId>\n            <artifactId>y</artifactId>\n        </dependency>\n";
        let out = insert_into_span(POM_BASIC, &span, frag);
        assert!(out.contains("<artifactId>y</artifactId>"));
        assert!(out.contains("    </dependencies>\n"));
    }
}
