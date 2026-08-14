//! Multi-language Change Risk Anti-Patterns analysis.

pub mod analysis;
pub mod baseline;
pub mod config;
pub mod coverage;
pub mod merge;
pub mod model;
pub mod report;
pub mod score;

pub use analysis::{Analysis, analyze_paths, analyze_tree};
pub use coverage::{CoverageMap, parse_coverage_files};
pub use merge::{MergeResult, merge, merge_selected};
pub use model::{Entry, Language, MissingCoveragePolicy};
