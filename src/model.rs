use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

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
    #[value(alias = "tf")]
    Terraform,
}

impl Language {
    pub const ALL: [Self; 7] = [
        Self::JavaScript,
        Self::TypeScript,
        Self::Python,
        Self::Go,
        Self::Rust,
        Self::Java,
        Self::Terraform,
    ];

    const NAMES: [&'static str; 7] = [
        "javascript",
        "typescript",
        "python",
        "go",
        "rust",
        "java",
        "terraform",
    ];

    const ALIASES: [(&'static str, Self); 12] = [
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
        ("terraform", Self::Terraform),
        ("tf", Self::Terraform),
    ];

    const EXTENSIONS: [(&'static [&'static str], Self); 7] = [
        (&["js", "jsx", "mjs", "cjs"], Self::JavaScript),
        (&["ts", "tsx", "mts", "cts"], Self::TypeScript),
        (&["py"], Self::Python),
        (&["go"], Self::Go),
        (&["rs"], Self::Rust),
        (&["java"], Self::Java),
        (&["tf"], Self::Terraform),
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
        if is_terraform_test(path) {
            return None;
        }
        let extension = path.extension().and_then(std::ffi::OsStr::to_str)?;
        Self::EXTENSIONS
            .iter()
            .find_map(|(extensions, language)| extensions.contains(&extension).then_some(*language))
    }
}

fn is_terraform_test(path: &std::path::Path) -> bool {
    path.file_name()
        .and_then(std::ffi::OsStr::to_str)
        .is_some_and(|name| name.ends_with(".tftest.hcl"))
}

impl fmt::Display for Language {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CoverageBasis {
    Line,
    Statement,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MetricKind {
    Crap,
    Complexity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum MissingCoveragePolicy {
    Pessimistic,
    Optimistic,
    Skip,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LineRange {
    pub start: usize,
    pub end: usize,
}

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub language: Language,
    pub file: PathBuf,
    pub symbol: String,
    pub start_line: usize,
    pub end_line: usize,
    pub metric: MetricKind,
    pub complexity: f64,
    pub coverage: Option<f64>,
    pub coverage_basis: Option<CoverageBasis>,
    pub crap: Option<f64>,
    pub score: f64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub uncovered: Vec<LineRange>,
}

#[cfg(test)]
mod tests {
    use super::*;

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
            ("terraform", Language::Terraform),
            ("tf", Language::Terraform),
        ];

        for (value, expected) in cases {
            assert_eq!(value.parse::<Language>(), Ok(expected));
            assert_eq!(value.to_ascii_uppercase().parse::<Language>(), Ok(expected));
        }
        assert_eq!(
            "ruby".parse::<Language>(),
            Err("unknown language: ruby".into())
        );
    }
}
