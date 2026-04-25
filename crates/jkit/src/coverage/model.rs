use serde::Serialize;

/// Raw parsed method data from XML (before line attribution).
pub struct ParsedMethod {
    pub name: String,
    pub start_line: u32,
    pub line_missed: u32,
    pub line_covered: u32,
    /// Total complexity (missed + covered from COMPLEXITY counter).
    pub complexity: u32,
    /// Missed line numbers (attributed from sourcefile after parsing).
    pub missed_lines: Vec<u32>,
}

/// Raw parsed class data from XML.
pub struct ParsedClass {
    pub class_name: String,
    pub source_file: String,
    pub methods: Vec<ParsedMethod>,
    /// Class-level LINE counter (mirrors what JaCoCo aggregates to bundle level).
    pub line_missed: u32,
    pub line_covered: u32,
}

/// Final output entry per method.
#[derive(Debug, Serialize)]
pub struct FilteredMethod {
    pub class: String,
    pub source_file: String,
    pub method: String,
    pub score: f64,
    pub missed_lines: Vec<u32>,
}

/// Aggregate line coverage summary for the whole report.
#[derive(Debug, Serialize)]
pub struct CoverageSummary {
    pub line_coverage_pct: f64,
    pub lines_covered: u32,
    pub lines_missed: u32,
}

/// Full report output (used with --summary flag).
#[derive(Debug, Serialize)]
pub struct Report {
    pub summary: CoverageSummary,
    pub methods: Vec<FilteredMethod>,
}
