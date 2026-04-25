use super::model::ParsedMethod;

/// Returns true if the method should be skipped as trivial.
pub fn is_trivial(name: &str) -> bool {
    name == "<init>" || name.starts_with("get") || name.starts_with("set")
}

/// Returns true if the method has zero missed lines (fully covered).
pub fn is_fully_covered(method: &ParsedMethod) -> bool {
    method.line_missed == 0
}
