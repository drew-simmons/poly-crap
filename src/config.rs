use crate::baseline::DEFAULT_EPSILON;
use crate::model::{Language, MissingCoveragePolicy};
use crate::report::{OutputFormat, SortOrder};
use crate::score::DEFAULT_THRESHOLD;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

pub const DEFAULT_EXCLUDES: &[&str] = &[
    "node_modules/**",
    "**/node_modules/**",
    "target/**",
    "**/target/**",
    "vendor/**",
    "**/vendor/**",
    ".terraform/**",
    "**/.terraform/**",
    "dist/**",
    "**/dist/**",
    "build/**",
    "**/build/**",
    "tests/**",
    "**/tests/**",
    "test/**",
    "**/test/**",
    "__tests__/**",
    "**/__tests__/**",
    "src/test/**",
    "**/src/test/**",
    "benches/**",
    "**/benches/**",
    "examples/**",
    "**/examples/**",
    "*_test.go",
    "**/*_test.go",
    "*.test.*",
    "**/*.test.*",
    "*.spec.*",
    "**/*.spec.*",
    "*.test.js",
    "**/*.test.js",
    "*.test.ts",
    "**/*.test.ts",
    "*.spec.js",
    "**/*.spec.js",
    "*.spec.ts",
    "**/*.spec.ts",
    "test_*.py",
    "**/test_*.py",
    "*_test.py",
    "**/*_test.py",
];

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Config {
    pub languages: Option<Vec<Language>>,
    #[serde(default)]
    pub coverage: Vec<PathBuf>,
    pub threshold: Option<f64>,
    pub missing: Option<MissingCoveragePolicy>,
    #[serde(default)]
    pub exclude: Vec<String>,
    pub default_excludes: Option<Vec<String>>,
    #[serde(default)]
    pub allow: Vec<String>,
    pub min: Option<f64>,
    pub top: Option<usize>,
    pub sort: Option<SortOrder>,
    pub format: Option<OutputFormat>,
    pub summary: Option<bool>,
    pub fail_above: Option<bool>,
    pub fail_regression: Option<bool>,
    pub epsilon: Option<f64>,
    pub jobs: Option<usize>,
}

#[derive(Debug)]
pub struct EffectiveConfig {
    pub languages: Vec<Language>,
    pub coverage: Vec<PathBuf>,
    pub threshold: f64,
    pub missing: MissingCoveragePolicy,
    pub excludes: Vec<String>,
    pub allow: Vec<String>,
    pub min: Option<f64>,
    pub top: Option<usize>,
    pub sort: SortOrder,
    pub format: OutputFormat,
    pub summary: bool,
    pub fail_above: bool,
    pub fail_regression: bool,
    pub epsilon: f64,
    pub jobs: Option<usize>,
}

impl EffectiveConfig {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        config: Config,
        cli_languages: Vec<Language>,
        cli_coverage: Vec<PathBuf>,
        cli_threshold: Option<f64>,
        cli_missing: Option<MissingCoveragePolicy>,
        cli_exclude: Vec<String>,
        no_default_excludes: bool,
        cli_allow: Vec<String>,
        cli_min: Option<f64>,
        cli_top: Option<usize>,
        cli_sort: Option<SortOrder>,
        cli_format: Option<OutputFormat>,
        cli_summary: bool,
        cli_fail_above: bool,
        cli_fail_regression: bool,
        cli_epsilon: Option<f64>,
        cli_jobs: Option<usize>,
    ) -> Self {
        let languages = if cli_languages.is_empty() {
            config.languages.unwrap_or_else(|| Language::ALL.to_vec())
        } else {
            cli_languages
        };
        let coverage = if cli_coverage.is_empty() {
            config.coverage
        } else {
            cli_coverage
        };
        let mut excludes = if no_default_excludes {
            Vec::new()
        } else {
            config
                .default_excludes
                .unwrap_or_else(|| DEFAULT_EXCLUDES.iter().map(ToString::to_string).collect())
        };
        excludes.extend(config.exclude);
        excludes.extend(cli_exclude);
        let mut allow = config.allow;
        allow.extend(cli_allow);
        Self {
            languages,
            coverage,
            threshold: cli_threshold
                .or(config.threshold)
                .unwrap_or(DEFAULT_THRESHOLD),
            missing: cli_missing
                .or(config.missing)
                .unwrap_or(MissingCoveragePolicy::Pessimistic),
            excludes,
            allow,
            min: cli_min.or(config.min),
            top: cli_top.or(config.top),
            sort: cli_sort.or(config.sort).unwrap_or_default(),
            format: cli_format.or(config.format).unwrap_or_default(),
            summary: cli_summary || config.summary.unwrap_or(false),
            fail_above: cli_fail_above || config.fail_above.unwrap_or(false),
            fail_regression: cli_fail_regression || config.fail_regression.unwrap_or(false),
            epsilon: cli_epsilon.or(config.epsilon).unwrap_or(DEFAULT_EPSILON),
            jobs: cli_jobs.or(config.jobs),
        }
    }
}

pub fn load(start: &Path) -> Result<Config> {
    let mut directory = if start.is_file() {
        start.parent().unwrap_or(start)
    } else {
        start
    };
    loop {
        let path = directory.join(".poly-crap.toml");
        if path.exists() {
            let raw = std::fs::read_to_string(&path)
                .with_context(|| format!("reading {}", path.display()))?;
            return toml::from_str(&raw).with_context(|| format!("parsing {}", path.display()));
        }
        match directory.parent() {
            Some(parent) => directory = parent,
            None => return Ok(Config::default()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unknown_keys() {
        let error = toml::from_str::<Config>("typo = true").unwrap_err();
        assert!(error.to_string().contains("unknown field"));
    }

    #[test]
    fn parses_language_and_policy() {
        let config: Config = toml::from_str(
            "languages = [\"rust\", \"python\"]\nmissing = \"skip\"\nthreshold = 12.0",
        )
        .unwrap();
        assert_eq!(
            config.languages.unwrap(),
            [Language::Rust, Language::Python]
        );
        assert_eq!(config.missing, Some(MissingCoveragePolicy::Skip));
    }
}
