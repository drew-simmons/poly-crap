/// Default CRAP threshold.
pub const DEFAULT_THRESHOLD: f64 = 5.0;

/// Compute the Change Risk Anti-Patterns score.
#[must_use]
pub fn crap(complexity: f64, coverage: f64) -> f64 {
    let uncovered = 1.0 - coverage.clamp(0.0, 100.0) / 100.0;
    complexity.powi(2) * uncovered.powi(3) + complexity
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn published_examples_match() {
        assert_eq!(crap(1.0, 100.0), 1.0);
        assert_eq!(crap(4.0, 50.0), 6.0);
        assert_eq!(crap(6.0, 0.0), 42.0);
    }
}
