use jkit::coverage::{filter, parser, scorer};

const SAMPLE_XML: &str = include_str!("fixtures/sample.xml");

fn process_sample(min_score: f64) -> Vec<jkit::coverage::model::FilteredMethod> {
    let classes = parser::parse(SAMPLE_XML).expect("parse sample.xml");
    let mut out = Vec::new();
    for class in &classes {
        for m in &class.methods {
            if filter::is_trivial(&m.name) || filter::is_fully_covered(m) {
                continue;
            }
            let total = (m.line_missed + m.line_covered) as usize;
            let s = scorer::score(m.complexity, m.missed_lines.len(), total);
            if s < min_score {
                continue;
            }
            out.push(jkit::coverage::model::FilteredMethod {
                class: class.class_name.clone(),
                source_file: class.source_file.clone(),
                method: m.name.clone(),
                score: s,
                missed_lines: m.missed_lines.clone(),
            });
        }
    }
    out.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    out
}

#[test]
fn constructor_is_filtered() {
    let m = process_sample(0.0);
    let names: Vec<&str> = m.iter().map(|m| m.method.as_str()).collect();
    assert!(!names.contains(&"<init>"));
}

#[test]
fn getter_is_filtered() {
    let m = process_sample(0.0);
    let names: Vec<&str> = m.iter().map(|m| m.method.as_str()).collect();
    assert!(!names.contains(&"getDeductionRule"));
}

#[test]
fn non_trivial_methods_present() {
    let m = process_sample(0.0);
    let names: Vec<&str> = m.iter().map(|m| m.method.as_str()).collect();
    assert!(names.contains(&"resourceType"));
    assert!(names.contains(&"startUseResourceActually"));
    assert!(names.contains(&"finishUseResource"));
}

#[test]
fn method_count() {
    assert_eq!(process_sample(0.0).len(), 3);
}

#[test]
fn scores_are_one() {
    for m in process_sample(0.0) {
        assert!((m.score - 1.0).abs() < 1e-9, "method {} score {}", m.method, m.score);
    }
}

#[test]
fn min_score_filter() {
    assert!(process_sample(1.1).is_empty());
}

#[test]
fn sorted_by_score_desc() {
    let m = process_sample(0.0);
    for w in m.windows(2) {
        assert!(w[0].score >= w[1].score);
    }
}

#[test]
fn scorer_unit() {
    assert!((scorer::score(2, 5, 10) - 1.0).abs() < 1e-9);
    assert!((scorer::score(3, 1, 3) - 1.0).abs() < 1e-9);
    assert_eq!(scorer::score(5, 0, 10), 0.0);
    assert_eq!(scorer::score(5, 5, 0), 0.0);
}

#[test]
fn filter_unit() {
    assert!(filter::is_trivial("<init>"));
    assert!(filter::is_trivial("getValue"));
    assert!(filter::is_trivial("setName"));
    assert!(!filter::is_trivial("calculate"));
    assert!(!filter::is_trivial("resourceType"));
}

#[test]
fn cli_top_k_limits_output() {
    let bin = env!("CARGO_BIN_EXE_jkit");
    let xml = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/sample.xml");
    let out = std::process::Command::new(bin)
        .args(["coverage", xml.to_str().unwrap(), "--top-k", "2"])
        .output()
        .expect("run jkit coverage");
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid json");
    assert_eq!(v.as_array().unwrap().len(), 2);
}

#[test]
fn cli_summary_envelope() {
    let bin = env!("CARGO_BIN_EXE_jkit");
    let xml = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/sample.xml");
    let out = std::process::Command::new(bin)
        .args(["coverage", xml.to_str().unwrap(), "--summary", "--top-k", "1"])
        .output()
        .expect("run jkit coverage --summary");
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid json");
    assert_eq!(v["methods"].as_array().unwrap().len(), 1);
    assert!(v["summary"].is_object());
}

#[test]
fn cli_iteration_state_emits_envelope_fields() {
    let bin = env!("CARGO_BIN_EXE_jkit");
    let xml = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/sample.xml");
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("state.json");
    let out = std::process::Command::new(bin)
        .args([
            "coverage",
            xml.to_str().unwrap(),
            "--iteration-state",
            state.to_str().unwrap(),
        ])
        .output()
        .expect("run jkit coverage --iteration-state");
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid json");
    assert_eq!(v["iteration"], 1);
    assert!(v["missed_lines_total"].is_number());
    assert_eq!(v["consecutive_no_progress"], 0);
    assert_eq!(v["should_stop"], false);
    assert!(state.exists());
}
