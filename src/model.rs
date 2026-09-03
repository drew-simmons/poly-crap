use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

/// A supported source language. The discriminant indexes parallel tables in
/// `analysis.rs` and here, so the order is part of the contract.
#[repr(usize)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    #[value(name = "javascript", alias = "js")]
    JavaScript,
    #[value(name = "typescript", alias = "ts")]
    TypeScript,
    #[value(alias = "py")]
    Python,
    Go,
    Rust,
    Java,
}

impl Language {
    pub const ALL: [Self; 6] = [
        Self::JavaScript,
        Self::TypeScript,
        Self::Python,
        Self::Go,
        Self::Rust,
        Self::Java,
    ];

    const NAMES: [&'static str; 6] = ["javascript", "typescript", "python", "go", "rust", "java"];

    const ALIASES: [(&'static str, Self); 10] = [
        ("javascript", Self::JavaScript),
        ("js", Self::JavaScript),
        ("typescript", Self::TypeScript),
        ("ts", Self::TypeScript),
        ("python", Self::Python),
        ("py", Self::Python),
        ("go", Self::Go),
        ("rust", Self::Rust),
        ("rs", Self::Rust),
        ("java", Self::Java),
    ];

    const EXTENSIONS: [(&'static [&'static str], Self); 6] = [
        (&["js", "jsx", "mjs", "cjs"], Self::JavaScript),
        (&["ts", "tsx", "mts", "cts"], Self::TypeScript),
        (&["py"], Self::Python),
        (&["go"], Self::Go),
        (&["rs"], Self::Rust),
        (&["java"], Self::Java),
    ];

    #[must_use]
    pub const fn index(self) -> usize {
        self as usize
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        Self::NAMES[self.index()]
    }

    #[must_use]
    pub fn from_path(path: &std::path::Path) -> Option<Self> {
        let extension = path.extension().and_then(std::ffi::OsStr::to_str)?;
        Self::EXTENSIONS
            .iter()
            .find_map(|(extensions, language)| extensions.contains(&extension).then_some(*language))
    }
}

impl fmt::Display for Language {
    /// Honors width and alignment, so a `{:<10}` column lines up. `write_str`
    /// would ignore them and jag the human table.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(self.as_str())
    }
}

impl FromStr for Language {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let normalized = value.to_ascii_lowercase();
        Self::ALIASES
            .iter()
            .find_map(|(alias, language)| (*alias == normalized).then_some(*language))
            .ok_or_else(|| format!("unknown language: {value}"))
    }
}

/// What a coverage report counts: lines for LCOV, Cobertura, and JaCoCo, or
/// statements, weighted by count, for a Go cover profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CoverageBasis {
    Line,
    Statement,
}

/// What to do with a function that has no coverage data at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum MissingCoveragePolicy {
    /// Score it as 0% covered. The default.
    Pessimistic,
    /// Score it as 100% covered.
    Optimistic,
    /// Leave it out of the report and the gates.
    Skip,
}

/// An inclusive range of one-based line numbers.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LineRange {
    pub start: usize,
    pub end: usize,
}

/// A function as the parser found it, before coverage is known.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeUnit {
    pub language: Language,
    pub file: PathBuf,
    pub symbol: String,
    pub start_line: usize,
    pub end_line: usize,
    pub complexity: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    pub kind: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScopeDiagnostics {
    pub candidate_files: usize,
    pub parsed_files: usize,
    pub analyzed_files: usize,
    pub coverage_files: usize,
    pub matched_files: usize,
    pub source_only_count: usize,
    pub coverage_only_count: usize,
    pub warning_count: usize,
    pub source_only_examples: Vec<PathBuf>,
    pub coverage_only_examples: Vec<PathBuf>,
    pub warnings: Vec<Diagnostic>,
}

/// A scored function: one row of a report, and one element of the JSON
/// `entries` array described by `schemas/report-v1.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub language: Language,
    pub file: PathBuf,
    /// The qualified name, such as `Class.method` or `module::function`.
    pub symbol: String,
    pub start_line: usize,
    pub end_line: usize,
    pub complexity: f64,
    /// Percent of executable lines a test ran, or `None` when no coverage
    /// data covered the function and the missing policy supplied the score.
    pub coverage: Option<f64>,
    pub coverage_basis: Option<CoverageBasis>,
    pub score: f64,
    /// Lines inside the function that no test ran.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub uncovered: Vec<LineRange>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_pads_to_the_requested_width() {
        assert_eq!(format!("{:<10}|", Language::Go), "go        |");
        assert_eq!(format!("{}", Language::TypeScript), "typescript");
    }

    #[test]
    fn parses_language_names_and_aliases() {
        let cases = [
            ("javascript", Language::JavaScript),
            ("js", Language::JavaScript),
            ("typescript", Language::TypeScript),
            ("ts", Language::TypeScript),
            ("python", Language::Python),
            ("py", Language::Python),
            ("go", Language::Go),
            ("rust", Language::Rust),
            ("rs", Language::Rust),
            ("java", Language::Java),
        ];

        for (value, expected) in cases {
            assert_eq!(value.parse::<Language>(), Ok(expected));
            assert_eq!(value.to_ascii_uppercase().parse::<Language>(), Ok(expected));
        }
        assert_eq!(
            "ruby".parse::<Language>(),
            Err("unknown language: ruby".into())
        );
        assert_eq!(
            "terraform".parse::<Language>(),
            Err("unknown language: terraform".into())
        );
    }
}
