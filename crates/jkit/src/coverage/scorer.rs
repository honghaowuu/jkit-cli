/// Compute the priority score for a method.
///
/// score = complexity × (missed_lines / total_lines)
pub fn score(complexity: u32, missed: usize, total: usize) -> f64 {
    if total == 0 {
        return 0.0;
    }
    complexity as f64 * (missed as f64 / total as f64)
}
