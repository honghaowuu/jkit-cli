use std::collections::HashMap;

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

use super::model::{ParsedClass, ParsedMethod};

struct MethodBuilder {
    name: String,
    start_line: u32,
    line_missed: u32,
    line_covered: u32,
    complexity: u32,
}

struct LineDatum {
    nr: u32,
    mi: u32,
    mb: u32,
}

fn get_attr(e: &BytesStart, name: &[u8]) -> Option<String> {
    e.attributes()
        .filter_map(|a| a.ok())
        .find(|a| a.key.as_ref() == name)
        .and_then(|a| a.unescape_value().ok().map(|v| v.into_owned()))
}

fn parse_u32(e: &BytesStart, name: &[u8]) -> u32 {
    get_attr(e, name).and_then(|v| v.parse().ok()).unwrap_or(0)
}

/// Assign missed line numbers to methods based on sourcefile line data.
///
/// Methods are sorted by start_line; each method owns lines from its start
/// up to (but not including) the next method's start. The last method is
/// open-ended.
fn assign_lines(mut methods: Vec<ParsedMethod>, lines: &[LineDatum]) -> Vec<ParsedMethod> {
    methods.sort_by_key(|m| m.start_line);
    for line in lines {
        // Find the last method whose start_line <= line.nr
        let idx = methods.partition_point(|m| m.start_line <= line.nr);
        if idx > 0 && (line.mi > 0 || line.mb > 0) {
            methods[idx - 1].missed_lines.push(line.nr);
        }
    }
    methods
}

pub fn parse(xml: &str) -> Result<Vec<ParsedClass>, Box<dyn std::error::Error>> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut result: Vec<ParsedClass> = Vec::new();

    // Per-package accumulators: (class_name, source_file, methods, line_missed, line_covered)
    let mut pkg_classes: Vec<(String, String, Vec<ParsedMethod>, u32, u32)> = Vec::new();
    let mut sf_lines: HashMap<String, Vec<LineDatum>> = HashMap::new();

    // Current class
    let mut cur_class_name: Option<String> = None;
    let mut cur_source_file: Option<String> = None;
    let mut cur_methods: Vec<ParsedMethod> = Vec::new();
    let mut cur_class_line_missed: u32 = 0;
    let mut cur_class_line_covered: u32 = 0;

    // Current method
    let mut cur_method: Option<MethodBuilder> = None;
    let mut in_method = false;

    // Current sourcefile
    let mut cur_sf_name: Option<String> = None;
    let mut cur_sf_lines: Vec<LineDatum> = Vec::new();
    let mut in_sourcefile = false;

    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf)? {
            // Non-empty opening tags: set state, expect a matching End event.
            Event::Start(ref e) => {
                match e.name().as_ref() {
                    b"class" => {
                        cur_class_name =
                            get_attr(e, b"name").map(|n| n.replace('/', "."));
                        cur_source_file = get_attr(e, b"sourcefilename");
                        cur_methods = Vec::new();
                        cur_class_line_missed = 0;
                        cur_class_line_covered = 0;
                    }
                    b"method" if cur_class_name.is_some() && !in_sourcefile => {
                        let name = get_attr(e, b"name").unwrap_or_default();
                        let start_line = parse_u32(e, b"line");
                        cur_method = Some(MethodBuilder {
                            name,
                            start_line,
                            line_missed: 0,
                            line_covered: 0,
                            complexity: 0,
                        });
                        in_method = true;
                    }
                    b"sourcefile" => {
                        cur_sf_name = get_attr(e, b"name");
                        cur_sf_lines = Vec::new();
                        in_sourcefile = true;
                    }
                    _ => {}
                }
            }
            // Self-closing tags: process fully in place, no End event follows.
            Event::Empty(ref e) => {
                match e.name().as_ref() {
                    b"class" => {
                        // Self-closing class (e.g. interface with no body) — record
                        // it with zero line counters.
                        let class_name = get_attr(e, b"name").map(|n| n.replace('/', "."));
                        let sf = get_attr(e, b"sourcefilename");
                        if let (Some(cn), Some(s)) = (class_name, sf) {
                            pkg_classes.push((cn, s, Vec::new(), 0, 0));
                        }
                    }
                    b"counter" if in_method => {
                        if let Some(ref mut m) = cur_method {
                            let typ = get_attr(e, b"type").unwrap_or_default();
                            let missed = parse_u32(e, b"missed");
                            let covered = parse_u32(e, b"covered");
                            match typ.as_str() {
                                "LINE" => {
                                    m.line_missed = missed;
                                    m.line_covered = covered;
                                }
                                "COMPLEXITY" => {
                                    m.complexity = missed + covered;
                                }
                                _ => {}
                            }
                        }
                    }
                    // Class-level LINE counter (outside any method, not in sourcefile).
                    // This is what JaCoCo aggregates up to the bundle level, so it
                    // matches what the Maven JaCoCo plugin reports.
                    b"counter" if cur_class_name.is_some() && !in_sourcefile => {
                        if get_attr(e, b"type").as_deref() == Some("LINE") {
                            cur_class_line_missed = parse_u32(e, b"missed");
                            cur_class_line_covered = parse_u32(e, b"covered");
                        }
                    }
                    b"sourcefile" => {
                        // Self-closing sourcefile has no <line> children; just record
                        // it as an empty entry so the lookup still works.
                        if let Some(name) = get_attr(e, b"name") {
                            sf_lines.entry(name).or_default();
                        }
                        // Do NOT set in_sourcefile — there is no closing tag to reset it.
                    }
                    b"line" if in_sourcefile => {
                        let nr = parse_u32(e, b"nr");
                        let mi = parse_u32(e, b"mi");
                        let mb = parse_u32(e, b"mb");
                        cur_sf_lines.push(LineDatum { nr, mi, mb });
                    }
                    _ => {}
                }
            }
            Event::End(ref e) => {
                match e.name().as_ref() {
                    b"method" if in_method => {
                        if let Some(m) = cur_method.take() {
                            cur_methods.push(ParsedMethod {
                                name: m.name,
                                start_line: m.start_line,
                                line_missed: m.line_missed,
                                line_covered: m.line_covered,
                                complexity: m.complexity,
                                missed_lines: Vec::new(),
                            });
                        }
                        in_method = false;
                    }
                    b"class" => {
                        if let (Some(class_name), Some(sf)) =
                            (cur_class_name.take(), cur_source_file.take())
                        {
                            pkg_classes.push((
                                class_name,
                                sf,
                                std::mem::take(&mut cur_methods),
                                cur_class_line_missed,
                                cur_class_line_covered,
                            ));
                        }
                    }
                    b"sourcefile" => {
                        if let Some(sf_name) = cur_sf_name.take() {
                            sf_lines.insert(sf_name, std::mem::take(&mut cur_sf_lines));
                        }
                        in_sourcefile = false;
                    }
                    b"package" => {
                        // Correlate sourcefile lines to class methods
                        for (class_name, sf, methods, line_missed, line_covered) in pkg_classes.drain(..) {
                            let methods_with_lines = if let Some(lines) = sf_lines.get(&sf) {
                                assign_lines(methods, lines)
                            } else {
                                methods
                            };
                            result.push(ParsedClass {
                                class_name,
                                source_file: sf,
                                methods: methods_with_lines,
                                line_missed,
                                line_covered,
                            });
                        }
                        sf_lines.clear();
                    }
                    _ => {}
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }

    // Handle any remaining package data (e.g. no closing <package> tag)
    if !pkg_classes.is_empty() {
        for (class_name, sf, methods, line_missed, line_covered) in pkg_classes.drain(..) {
            let methods_with_lines = if let Some(lines) = sf_lines.get(&sf) {
                assign_lines(methods, lines)
            } else {
                methods
            };
            result.push(ParsedClass {
                class_name,
                source_file: sf,
                methods: methods_with_lines,
                line_missed,
                line_covered,
            });
        }
    }

    Ok(result)
}
