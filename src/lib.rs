//! Multi-language Change Risk Anti-Patterns analysis.
//!
//! Poly-crap scores every function in a codebase by cyclomatic complexity
//! against test coverage, using the CRAP metric in [`score::crap`]. The
//! `poly-crap` binary is the supported interface. This crate exposes the
//! stages it runs so another tool can embed them:
//!
//! 1. [`config::load`] reads `.poly-crap.toml`, and [`config::EffectiveConfig`]
//!    layers command-line [`config::Overrides`] over it.
//! 2. [`analyze_tree`] parses the source under a root into
//!    [`model::CodeUnit`] values, one per function.
//! 3. [`parse_coverage_files`] reads LCOV, Cobertura XML, JaCoCo XML, or Go
//!    cover profiles into a [`CoverageMap`].
//! 4. [`merge()`] joins the two into scored [`Entry`] values.
//! 5. [`report::gate_entries`] builds the set the exit code is decided on, the
//!    `render_*` functions in [`report`] write human, JSON, or SARIF output,
//!    and [`baseline::compare`] diffs a run against an earlier JSON report.
//!    Human output is laid out by [`table`] and colored through a
//!    [`style::Theme`]; the machine formats never see one.
//!
//! ```rust,no_run
//! use poly_crap::config::{Config, EffectiveConfig, Overrides};
//! use poly_crap::{analyze_tree, merge, parse_coverage_files};
//! use std::path::Path;
//!
//! # fn main() -> anyhow::Result<()> {
//! let config = EffectiveConfig::new(Config::default(), Overrides::default());
//! let analysis = analyze_tree(Path::new("."), &config.languages, &config.excludes)?;
//! let coverage = parse_coverage_files(&config.coverage)?;
//! let merged = merge(analysis, &coverage, config.missing);
//! for entry in merged.entries.iter().filter(|entry| entry.score > config.threshold) {
//!     println!("{} {:.1}", entry.symbol, entry.score);
//! }
//! # Ok(())
//! # }
//! ```
//!
//! [`analyze_tree`] scans whatever it is given. The built-in excludes in
//! [`config::DEFAULT_EXCLUDES`] reach it only through an `EffectiveConfig`, so
//! a caller that passes its own list scans `node_modules` and `target` too.
//! The JSON schemas in `schemas/` are the published contract; the Rust types
//! here are not, and may change between minor versions.

pub mod analysis;
pub mod baseline;
pub mod config;
pub mod coverage;
pub mod merge;
pub mod model;
pub mod report;
pub mod score;
pub mod style;
pub mod table;

pub use analysis::{Analysis, analyze_paths, analyze_tree};
pub use coverage::{CoverageMap, discover_reports, parse_coverage_files};
pub use merge::{MergeResult, merge, merge_selected};
pub use model::{Entry, Language, MissingCoveragePolicy};
