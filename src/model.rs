use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

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

    #[must_use]
    pub fn from_path(path: &std::path::Path) -> Option<Self> {
        let name = path.file_name()?.to_str()?;
        let extension = path.extension()?.to_str()?;
        match extension {
            "js" | "jsx" | "mjs" | "cjs" => Some(Self::JavaScript),
            "ts" | "tsx" | "mts" | "cts" => Some(Self::TypeScript),
            "py" => Some(Self::Python),
            "go" => Some(Self::Go),
            "rs" => Some(Self::Rust),
            "java" => Some(Self::Java),
            "tf" if !name.ends_with(".tftest.hcl") => Some(Self::Terraform),
            _ => None,
        }
    }
}

impl fmt::Display for Language {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::JavaScript => "javascript",
            Self::TypeScript => "typescript",
            Self::Python => "python",
            Self::Go => "go",
            Self::Rust => "rust",
            Self::Java => "java",
            Self::Terraform => "terraform",
        };
        f.write_str(value)
    }
}

impl FromStr for Language {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "javascript" | "js" => Ok(Self::JavaScript),
            "typescript" | "ts" => Ok(Self::TypeScript),
            "python" | "py" => Ok(Self::Python),
            "go" => Ok(Self::Go),
            "rust" | "rs" => Ok(Self::Rust),
            "java" => Ok(Self::Java),
            "terraform" | "tf" => Ok(Self::Terraform),
            _ => Err(format!("unknown language: {value}")),
        }
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
