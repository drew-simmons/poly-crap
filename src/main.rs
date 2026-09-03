mod git_diff;

use anyhow::{Context, Result, bail};
use clap::{Parser, error::ErrorKind};
use poly_crap::baseline;
use poly_crap::config::{self, EffectiveConfig, Overrides};
use poly_crap::model::{Entry, Language, MissingCoveragePolicy, ScopeDiagnostics};
use poly_crap::report::{self, OutputFormat, Presentation, SortOrder};
use poly_crap::style::{self, ColorMode, Theme};
use poly_crap::{
    Analysis, analyze_paths, analyze_tree, discover_reports, merge, merge_selected,
    parse_coverage_files,
};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::SystemTime;

#[derive(Debug, Parser)]
#[command(
    name = "poly-crap",
    version,
    about = "Find risky, untested complexity across polyglot codebases"
)]
struct Cli {
    /// Root directory to scan.
    #[arg(long, default_value = ".")]
    path: PathBuf,

    /// Language to scan; repeat to select more than one. Scans all by default.
    #[arg(long, value_enum)]
    language: Vec<Language>,

    /// LCOV, Go cover profile, or JaCoCo XML file. May be repeated.
    /// When omitted, default report locations under --path are searched.
    #[arg(long, value_name = "FILE")]
    coverage: Vec<PathBuf>,

    /// Do not search default report locations when no coverage is given.
    #[arg(long)]
    no_auto_coverage: bool,

    /// CRAP score above which a function fails the absolute gate.
    #[arg(long)]
    threshold: Option<f64>,

    /// Policy for functions that have no matching coverage data.
    #[arg(long, value_enum)]
    missing: Option<MissingCoveragePolicy>,

    /// Source path glob to skip. May be repeated.
    #[arg(long, value_name = "GLOB")]
    exclude: Vec<String>,

    /// Disable built-in dependency, build, generated, and test exclusions.
    #[arg(long)]
    no_default_excludes: bool,

    /// Symbol or path glob to suppress. May be repeated.
    #[arg(long, value_name = "GLOB")]
    allow: Vec<String>,

    /// Hide entries whose CRAP score is below this value. Human output otherwise
    /// lists only entries over the threshold; `--min 0` lists them all.
    /// Does not affect the gates.
    #[arg(long)]
    min: Option<f64>,

    /// Show at most this many entries. Does not affect the gates.
    #[arg(long)]
    top: Option<usize>,

    /// Sort entries by score or by source file.
    #[arg(long, value_enum)]
    sort: Option<SortOrder>,

    /// Output format.
    #[arg(long, value_enum)]
    format: Option<OutputFormat>,

    /// Print aggregate human output without entry rows.
    #[arg(long)]
    summary: bool,

    /// Exit 1 when a function exceeds the CRAP threshold.
    #[arg(long)]
    fail_above: bool,

    /// JSON report from an earlier `poly-crap --format json` run.
    #[arg(long, value_name = "FILE")]
    baseline: Option<PathBuf>,

    /// Git revision used to limit analysis to changed functions.
    #[arg(long, value_name = "REV", conflicts_with = "baseline")]
    diff_base: Option<String>,

    /// Exit 1 when a CRAP score rises from the baseline.
    #[arg(long, requires = "baseline")]
    fail_regression: bool,

    /// Delta tolerance for baseline comparisons.
    #[arg(long, allow_negative_numbers = true)]
    epsilon: Option<f64>,

    /// Write the report to a file instead of stdout.
    #[arg(long, value_name = "FILE")]
    output: Option<PathBuf>,

    /// When to color output: auto, always, or never. Auto colors a terminal
    /// and honors NO_COLOR.
    #[arg(long, value_enum, default_value_t, value_name = "WHEN")]
    color: ColorMode,

    /// Maximum source parsing threads.
    #[arg(long)]
    jobs: Option<usize>,
}

impl Cli {
    /// Colors for the report. Under `auto`, a report written to a file gets
    /// none, whatever stdout is.
    fn stdout_theme(&self) -> Theme {
        Theme::resolve(self.color, || {
            self.output.is_none() && style::stream_wants_color(&std::io::stdout())
        })
    }

    /// Colors for warnings and notes, judged on stderr's own terminal, so
    /// `--format json | jq` keeps a colored stderr.
    fn stderr_theme(&self) -> Theme {
        Theme::resolve(self.color, || style::stream_wants_color(&std::io::stderr()))
    }
}

fn main() -> ExitCode {
    parse_and_run(Cli::try_parse())
}

fn parse_and_run(parsed: clap::error::Result<Cli>) -> ExitCode {
    match parsed {
        Ok(cli) => run_and_exit(cli),
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) =>
        {
            print!("{error}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            let _ = error.print();
            ExitCode::from(2)
        }
    }
}

fn run_and_exit(cli: Cli) -> ExitCode {
    let stderr = cli.stderr_theme();
    match run(cli) {
        Ok(true) => ExitCode::from(1),
        Ok(false) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{}", style::error(stderr, format!("{error:#}")));
            ExitCode::from(2)
        }
    }
}

fn run(cli: Cli) -> Result<bool> {
    let (rendered, gate_failed) = build_report(&cli)?;
    write_report(&rendered, cli.output.as_ref())?;
    Ok(gate_failed)
}

fn build_report(cli: &Cli) -> Result<(String, bool)> {
    let effective = effective_config(cli)?;
    validate(cli, &effective)?;
    let collected = collect_entries(cli, &effective)?;
    render_report(cli, &effective, collected)
}

struct Collected {
    entries: Vec<Entry>,
    diagnostics: ScopeDiagnostics,
    diff: Option<git_diff::GitDiff>,
}

fn collect_entries(cli: &Cli, config: &EffectiveConfig) -> Result<Collected> {
    let stderr = cli.stderr_theme();
    let (analysis, diff) = analyze_sources(cli, config)?;
    warn_diagnostics(&analysis, stderr);
    warn_stale_coverage(&config.coverage, &analysis, stderr);
    let merged = merge_sources(analysis, config, diff.is_some(), stderr)?;
    let entries = gate_entries(merged.entries, &cli.path, config)?;
    Ok(Collected {
        entries,
        diagnostics: merged.diagnostics,
        diff,
    })
}

fn effective_config(cli: &Cli) -> Result<EffectiveConfig> {
    let file_config = config::load(&cli.path)?;
    let overrides = Overrides {
        languages: cli.language.clone(),
        coverage: cli.coverage.clone(),
        threshold: cli.threshold,
        missing: cli.missing,
        exclude: cli.exclude.clone(),
        no_default_excludes: cli.no_default_excludes,
        allow: cli.allow.clone(),
        min: cli.min,
        top: cli.top,
        sort: cli.sort,
        format: cli.format,
        summary: cli.summary,
        fail_above: cli.fail_above,
        fail_regression: cli.fail_regression,
        epsilon: cli.epsilon,
        jobs: cli.jobs,
    };
    let mut effective = EffectiveConfig::new(file_config, overrides);
    discover_missing_coverage(cli, &mut effective);
    Ok(effective)
}

/// Fill in default-location reports when the CLI and the config name none.
fn discover_missing_coverage(cli: &Cli, config: &mut EffectiveConfig) {
    if cli.no_auto_coverage || !config.coverage.is_empty() {
        return;
    }
    config.coverage = discover_reports(&cli.path);
    note_discovery(&config.coverage, config.missing, cli.stderr_theme());
}

fn note_discovery(reports: &[PathBuf], missing: MissingCoveragePolicy, theme: Theme) {
    if reports.is_empty() {
        let policy = format!("{missing:?}").to_lowercase();
        let message = format!(
            "no coverage report found; every function follows the --missing policy ({policy})"
        );
        eprintln!("{}", style::warning(theme, message));
    } else {
        let listed: Vec<_> = reports
            .iter()
            .map(|path| path.display().to_string())
            .collect();
        let message = format!("using discovered coverage report(s): {}", listed.join(", "));
        eprintln!("{}", style::note(theme, message));
    }
}

fn analyze_sources(
    cli: &Cli,
    config: &EffectiveConfig,
) -> Result<(Analysis, Option<git_diff::GitDiff>)> {
    match &cli.diff_base {
        Some(base) => analyze_diff_sources(cli, config, base),
        None => analyze_all_sources(&cli.path, config),
    }
}

fn analyze_all_sources(
    path: &std::path::Path,
    config: &EffectiveConfig,
) -> Result<(Analysis, Option<git_diff::GitDiff>)> {
    let analysis = dispatch_analysis(path, config)?;
    ensure_any_parsed(&analysis)?;
    Ok((analysis, None))
}

fn analyze_diff_sources(
    cli: &Cli,
    config: &EffectiveConfig,
    base: &str,
) -> Result<(Analysis, Option<git_diff::GitDiff>)> {
    let diff = git_diff::discover(&cli.path, base)?;
    let selected = diff.selected_paths();
    let mut analysis = dispatch_selected_analysis(&cli.path, config, &selected)?;
    diff.retain_changed_units(&cli.path, &mut analysis);
    ensure_any_parsed(&analysis)?;
    Ok((analysis, Some(diff)))
}

fn dispatch_analysis(path: &std::path::Path, config: &EffectiveConfig) -> Result<Analysis> {
    if let Some(jobs) = config.jobs {
        analyze_in_pool(path, config, jobs)
    } else {
        analyze_tree(path, &config.languages, &config.excludes)
    }
}

fn analyze_in_pool(
    path: &std::path::Path,
    config: &EffectiveConfig,
    jobs: usize,
) -> Result<Analysis> {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(jobs)
        .build()
        .context("creating source parsing thread pool")?;
    pool.install(|| analyze_tree(path, &config.languages, &config.excludes))
}

fn dispatch_selected_analysis(
    path: &std::path::Path,
    config: &EffectiveConfig,
    selected: &[PathBuf],
) -> Result<Analysis> {
    if let Some(jobs) = config.jobs {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(jobs)
            .build()
            .context("creating source parsing thread pool")?;
        pool.install(|| analyze_paths(path, &config.languages, &config.excludes, selected))
    } else {
        analyze_paths(path, &config.languages, &config.excludes, selected)
    }
}

fn ensure_any_parsed(analysis: &Analysis) -> Result<()> {
    if analysis.candidate_files > 0 && analysis.parsed_files == 0 {
        bail!(
            "none of the {} candidate source files parsed successfully",
            analysis.candidate_files
        );
    }
    Ok(())
}

fn warn_diagnostics(analysis: &Analysis, theme: Theme) {
    for diagnostic in &analysis.diagnostics {
        eprintln!("{}", style::warning(theme, &diagnostic.message));
    }
}

/// Warn when a report predates the source it is meant to describe.
///
/// Auto-discovery makes it easy to score today's code against last week's
/// report. Modification times are a rough guide, but a report older than a
/// file it should cover cannot be right, and the mistake is otherwise silent.
fn warn_stale_coverage(reports: &[PathBuf], analysis: &Analysis, theme: Theme) {
    let newest = newest_modification(analysis.units.iter().map(|unit| unit.file.as_path()));
    for report in reports.iter().filter(|report| is_older(report, newest)) {
        let message = format!(
            "coverage report {} is older than the source it covers; regenerate it",
            report.display()
        );
        eprintln!("{}", style::warning(theme, message));
    }
}

fn newest_modification<'a>(paths: impl Iterator<Item = &'a Path>) -> Option<SystemTime> {
    paths.filter_map(modification_time).max()
}

fn is_older(report: &Path, newest: Option<SystemTime>) -> bool {
    match (modification_time(report), newest) {
        (Some(report), Some(source)) => report < source,
        _ => false,
    }
}

fn modification_time(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
}

fn merge_sources(
    analysis: Analysis,
    config: &EffectiveConfig,
    selected: bool,
    theme: Theme,
) -> Result<poly_crap::MergeResult> {
    let coverage = parse_coverage_files(&config.coverage)?;
    let merged = merge_for_scope(analysis, &coverage, config, selected);
    warn_coverage_scope(&merged, theme);
    Ok(merged)
}

fn merge_for_scope(
    analysis: Analysis,
    coverage: &poly_crap::CoverageMap,
    config: &EffectiveConfig,
    selected: bool,
) -> poly_crap::MergeResult {
    if selected {
        merge_selected(analysis, coverage, config.missing)
    } else {
        merge(analysis, coverage, config.missing)
    }
}

fn warn_coverage_scope(merged: &poly_crap::MergeResult, theme: Theme) {
    if merged.diagnostics.source_only_count > 0 || merged.diagnostics.coverage_only_count > 0 {
        let message = format!(
            "coverage scope mismatch: {}, {}",
            style::count(merged.diagnostics.source_only_count, "source-only file"),
            style::count(merged.diagnostics.coverage_only_count, "coverage-only file")
        );
        eprintln!("{}", style::warning(theme, message));
    }
}

/// Entries the gates and the summary counts see: `--allow` applied, nothing else.
fn gate_entries(
    entries: Vec<Entry>,
    root: &std::path::Path,
    config: &EffectiveConfig,
) -> Result<Vec<Entry>> {
    report::gate_entries(entries, &config.allow, root, config.sort)
}

fn render_report(
    cli: &Cli,
    config: &EffectiveConfig,
    collected: Collected,
) -> Result<(String, bool)> {
    if let Some(path) = &cli.baseline {
        render_delta_report(
            path,
            &cli.path,
            presentation(config, cli.stdout_theme()),
            config,
            collected.entries,
            collected.diagnostics,
        )
    } else {
        render_absolute_report(cli, config, collected)
    }
}

/// What the renderers read besides the rows.
fn presentation(config: &EffectiveConfig, theme: Theme) -> Presentation {
    Presentation {
        threshold: config.threshold,
        summary: config.summary,
        theme,
    }
}

fn render_delta_report(
    path: &std::path::Path,
    root: &std::path::Path,
    options: Presentation,
    config: &EffectiveConfig,
    entries: Vec<Entry>,
    diagnostics: ScopeDiagnostics,
) -> Result<(String, bool)> {
    let baseline_entries = report::filter_allowed(baseline::load(path)?, &config.allow, root)?;
    let mut delta = baseline::compare(entries, &baseline_entries, config.epsilon, diagnostics);
    let totals = report::DeltaTotals::new(&delta, config.threshold);
    let failed = delta_gate_failed(config, totals);
    report::limit_delta(
        &mut delta,
        report::RowFilter::explicit(config.min),
        config.top,
    );
    let rendered = report::render_delta(&delta, config.format, options, totals)?;
    Ok((rendered, failed))
}

/// Both gates apply to a baseline run. `--fail-regression` catches a score
/// that rose, and `--fail-above` catches a score over the threshold. A new
/// function has no baseline to rise from, so only the second can fail it.
fn delta_gate_failed(config: &EffectiveConfig, totals: report::DeltaTotals) -> bool {
    (config.fail_regression && totals.regressed()) || (config.fail_above && totals.exceeded())
}

fn render_absolute_report(
    cli: &Cli,
    config: &EffectiveConfig,
    collected: Collected,
) -> Result<(String, bool)> {
    let failed =
        config.fail_above && report::threshold_failed(&collected.entries, config.threshold);
    let selected_units = collected.entries.len();
    let totals = report::Totals::new(&collected.entries, config.threshold);
    let filter = report::RowFilter::for_report(config.format, config.min, config.threshold);
    let shown = report::apply_display_limits(collected.entries, filter, config.top, config.sort);
    let mut rendered = report::render_absolute(
        shown,
        collected.diagnostics,
        config.format,
        presentation(config, cli.stdout_theme()),
        totals,
    )?;
    add_diff_summary(
        &mut rendered,
        config.format,
        cli.diff_base.as_deref(),
        collected.diff,
        selected_units,
    );
    Ok((rendered, failed))
}

fn add_diff_summary(
    rendered: &mut String,
    format: OutputFormat,
    requested: Option<&str>,
    diff: Option<git_diff::GitDiff>,
    selected_units: usize,
) {
    let (OutputFormat::Human, Some(requested), Some(diff)) = (format, requested, diff) else {
        return;
    };
    *rendered = format!(
        "Git diff against {requested} from {}: {}, {} selected.\n{rendered}",
        diff.merge_base,
        style::count(diff.files.len(), "changed file"),
        style::count(selected_units, "function"),
    );
}

fn validate(cli: &Cli, config: &EffectiveConfig) -> Result<()> {
    validate_non_negative("threshold", config.threshold)?;
    validate_non_negative("epsilon", config.epsilon)?;
    validate_non_zero("jobs", config.jobs)?;
    validate_non_zero("top", config.top)?;
    validate_modes(cli, config)
}

fn validate_non_negative(name: &str, value: f64) -> Result<()> {
    if !value.is_finite() || value < 0.0 {
        bail!("--{name} must be a finite non-negative number");
    }
    Ok(())
}

fn validate_non_zero(name: &str, value: Option<usize>) -> Result<()> {
    if value == Some(0) {
        bail!("--{name} must be greater than zero");
    }
    Ok(())
}

fn validate_modes(cli: &Cli, config: &EffectiveConfig) -> Result<()> {
    validate_baseline_format(cli, config)?;
    validate_diff_mode(cli, config)?;
    validate_regression_mode(cli, config)
}

fn validate_diff_mode(cli: &Cli, config: &EffectiveConfig) -> Result<()> {
    if cli.diff_base.is_some() && config.fail_regression {
        bail!("--diff-base cannot be combined with fail-regression");
    }
    Ok(())
}

fn validate_baseline_format(cli: &Cli, config: &EffectiveConfig) -> Result<()> {
    if cli.baseline.is_some() && config.format == OutputFormat::Sarif {
        bail!("--baseline cannot be combined with --format sarif");
    }
    Ok(())
}

fn validate_regression_mode(cli: &Cli, config: &EffectiveConfig) -> Result<()> {
    if config.fail_regression && cli.baseline.is_none() {
        bail!("fail-regression in config requires --baseline");
    }
    Ok(())
}

fn write_report(report: &str, output: Option<&PathBuf>) -> Result<()> {
    match output {
        Some(path) => std::fs::write(path, format!("{report}\n"))
            .with_context(|| format!("writing report to {}", path.display())),
        None => {
            let mut stdout = std::io::stdout().lock();
            writeln!(stdout, "{report}").context("writing report to stdout")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    fn parse(values: &[&str]) -> clap::error::Result<Cli> {
        Cli::try_parse_from(std::iter::once("poly-crap").chain(values.iter().copied()))
    }

    #[test]
    fn accepts_language_aliases_and_repeated_coverage() {
        let cli = parse(&[
            "--language",
            "ts",
            "--language",
            "py",
            "--coverage",
            "a.info",
            "--coverage",
            "b.xml",
        ])
        .unwrap();
        assert_eq!(cli.language, [Language::TypeScript, Language::Python]);
        assert_eq!(cli.coverage.len(), 2);
    }

    #[test]
    fn help_contract_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn parse_errors_use_cli_exit_codes() {
        assert_eq!(parse_and_run(parse(&["--help"])), ExitCode::SUCCESS);
        assert_eq!(
            parse_and_run(parse(&["--not-an-option"])),
            ExitCode::from(2)
        );
    }

    fn set_modified(path: &Path, time: SystemTime) {
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(time)
            .unwrap();
    }

    #[test]
    fn a_report_older_than_the_source_is_stale() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("app.py");
        let report = dir.path().join("coverage.lcov");
        std::fs::write(&source, "def run(x):\n    return x\n").unwrap();
        std::fs::write(&report, "SF:app.py\nDA:1,1\nend_of_record\n").unwrap();
        let newest = newest_modification([source.as_path()].into_iter());
        assert!(newest.is_some());

        let old = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000);
        set_modified(&report, old);
        assert!(is_older(&report, newest));

        // A report at least as new as the source is fine. So is one that is
        // missing, or a scan that found no source to compare against.
        set_modified(&report, SystemTime::now());
        assert!(!is_older(&report, newest));
        assert!(!is_older(&dir.path().join("missing.lcov"), newest));
        assert!(!is_older(&report, None));
    }
}
