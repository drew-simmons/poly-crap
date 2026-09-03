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

/// The contents of `.poly-crap.toml`. Every key is optional, and an unknown
/// key is an error so a typo cannot silently do nothing. The README's options
/// reference gives each key's meaning.
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

/// The settings one run uses, after [`Overrides`] and the defaults are applied
/// to a [`Config`].
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

/// Values given on the command line, layered over a [`Config`] file.
///
/// Every field is empty or unset by default, so a caller sets only what it
/// means to override. Lists join the file's lists; a set option replaces the
/// file's value; a `true` flag is on whichever side set it.
#[derive(Debug, Clone, Default)]
pub struct Overrides {
    pub languages: Vec<Language>,
    pub coverage: Vec<PathBuf>,
    pub threshold: Option<f64>,
    pub missing: Option<MissingCoveragePolicy>,
    pub exclude: Vec<String>,
    /// Drop the built-in excludes and the file's `default-excludes` alike.
    pub no_default_excludes: bool,
    pub allow: Vec<String>,
    pub min: Option<f64>,
    pub top: Option<usize>,
    pub sort: Option<SortOrder>,
    pub format: Option<OutputFormat>,
    pub summary: bool,
    pub fail_above: bool,
    pub fail_regression: bool,
    pub epsilon: Option<f64>,
    pub jobs: Option<usize>,
}

impl EffectiveConfig {
    /// Resolve a run's settings: overrides win over the file, and the file over
    /// the defaults.
    #[must_use]
    pub fn new(config: Config, cli: Overrides) -> Self {
        let languages = prefer_cli(
            cli.languages,
            config.languages.unwrap_or_else(|| Language::ALL.to_vec()),
        );
        let coverage = prefer_cli(cli.coverage, config.coverage);
        let mut excludes = base_excludes(cli.no_default_excludes, config.default_excludes);
        excludes.extend(config.exclude);
        excludes.extend(cli.exclude);
        let mut allow = config.allow;
        allow.extend(cli.allow);
        Self {
            languages,
            coverage,
            threshold: cli
                .threshold
                .or(config.threshold)
                .unwrap_or(DEFAULT_THRESHOLD),
            missing: cli
                .missing
                .or(config.missing)
                .unwrap_or(MissingCoveragePolicy::Pessimistic),
            excludes,
            allow,
            min: cli.min.or(config.min),
            top: cli.top.or(config.top),
            sort: cli.sort.or(config.sort).unwrap_or_default(),
            format: cli.format.or(config.format).unwrap_or_default(),
            summary: enabled(cli.summary, config.summary),
            fail_above: enabled(cli.fail_above, config.fail_above),
            fail_regression: enabled(cli.fail_regression, config.fail_regression),
            epsilon: cli.epsilon.or(config.epsilon).unwrap_or(DEFAULT_EPSILON),
            jobs: cli.jobs.or(config.jobs),
        }
    }
}

fn prefer_cli<T>(cli: Vec<T>, configured: Vec<T>) -> Vec<T> {
    if cli.is_empty() { configured } else { cli }
}

fn base_excludes(no_defaults: bool, configured: Option<Vec<String>>) -> Vec<String> {
    if no_defaults {
        return Vec::new();
    }
    configured.unwrap_or_else(|| DEFAULT_EXCLUDES.iter().map(ToString::to_string).collect())
}

fn enabled(cli: bool, configured: Option<bool>) -> bool {
    [cli, configured.unwrap_or(false)].contains(&true)
}

pub fn load(start: &Path) -> Result<Config> {
    let path = search_root(start)
        .ancestors()
        .map(|directory| directory.join(".poly-crap.toml"))
        .find(|path| path.exists());
    path.map_or_else(|| Ok(Config::default()), |path| read_config(&path))
}

/// Absolute directory to search upward from.
///
/// `Path::ancestors` on a relative path such as the default `.` yields only
/// that path and `""`, so the search must start from an absolute path for
/// parent directories to be reached at all.
fn search_root(start: &Path) -> PathBuf {
    let directory = config_start(start);
    directory
        .canonicalize()
        .unwrap_or_else(|_| directory.to_path_buf())
}

fn config_start(start: &Path) -> &Path {
    if start.is_file() {
        start.parent().unwrap_or(start)
    } else {
        start
    }
}

fn read_config(path: &Path) -> Result<Config> {
    let raw =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    toml::from_str(&raw).with_context(|| format!("parsing {}", path.display()))
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

    #[test]
    fn overrides_win_over_the_file_and_lists_join() {
        let config: Config = toml::from_str(concat!(
            "threshold = 30.0\n",
            "languages = [\"go\"]\n",
            "exclude = [\"gen/**\"]\n",
            "allow = [\"a\"]\n",
            "fail-above = true\n",
        ))
        .unwrap();
        let effective = EffectiveConfig::new(
            config,
            Overrides {
                threshold: Some(5.0),
                languages: vec![Language::Rust],
                exclude: vec!["tmp/**".into()],
                allow: vec!["b".into()],
                ..Overrides::default()
            },
        );
        assert_eq!(effective.threshold, 5.0);
        assert_eq!(effective.languages, [Language::Rust]);
        assert!(effective.fail_above, "a flag set in the file stays on");
        assert!(
            effective
                .excludes
                .ends_with(&["gen/**".into(), "tmp/**".into()])
        );
        assert_eq!(effective.allow, ["a", "b"]);

        let defaults = EffectiveConfig::new(Config::default(), Overrides::default());
        assert_eq!(defaults.threshold, DEFAULT_THRESHOLD);
        assert_eq!(defaults.languages, Language::ALL);
        assert_eq!(defaults.excludes.len(), DEFAULT_EXCLUDES.len());
        assert!(
            EffectiveConfig::new(
                Config::default(),
                Overrides {
                    no_default_excludes: true,
                    ..Overrides::default()
                },
            )
            .excludes
            .is_empty()
        );
    }
}
