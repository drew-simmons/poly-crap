mod git_diff;

use anyhow::{Context, Result, bail};
use clap::{Parser, error::ErrorKind};
use poly_crap::baseline;
use poly_crap::config::{self, EffectiveConfig};
use poly_crap::model::{Entry, Language, MissingCoveragePolicy, ScopeDiagnostics};
use poly_crap::report::{self, OutputFormat, SortOrder};
use poly_crap::{
    Analysis, analyze_paths, analyze_tree, merge, merge_selected, parse_coverage_files,
};
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

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
    #[arg(long, value_name = "FILE")]
    coverage: Vec<PathBuf>,

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

    /// Hide entries whose primary score is below this value.
    #[arg(long)]
    min: Option<f64>,

    /// Show at most this many entries in each metric section.
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

    /// Exit 1 when a programming-language function exceeds the threshold.
    #[arg(long)]
    fail_above: bool,

    /// JSON report from an earlier `poly-crap --format json` run.
    #[arg(long, value_name = "FILE")]
    baseline: Option<PathBuf>,

    /// Git revision used to limit analysis to changed functions and blocks.
    #[arg(long, value_name = "REV", conflicts_with = "baseline")]
    diff_base: Option<String>,

    /// Exit 1 when CRAP or Terraform complexity rises from the baseline.
    #[arg(long, requires = "baseline")]
    fail_regression: bool,

    /// Delta tolerance for baseline comparisons.
    #[arg(long, allow_negative_numbers = true)]
    epsilon: Option<f64>,

    /// Write the report to a file instead of stdout.
    #[arg(long, value_name = "FILE")]
    output: Option<PathBuf>,

    /// Maximum source parsing threads.
    #[arg(long)]
    jobs: Option<usize>,
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
    match run(cli) {
        Ok(true) => ExitCode::from(1),
        Ok(false) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
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
    let (analysis, diff) = analyze_sources(cli, config)?;
    warn_diagnostics(&analysis);
    let merged = merge_sources(analysis, config, diff.is_some())?;
    let entries = filter_entries(merged.entries, config)?;
    Ok(Collected {
        entries,
        diagnostics: merged.diagnostics,
        diff,
    })
}

fn effective_config(cli: &Cli) -> Result<EffectiveConfig> {
    let file_config = config::load(&cli.path)?;
    Ok(EffectiveConfig::new(
        file_config,
        cli.language.clone(),
        cli.coverage.clone(),
        cli.threshold,
        cli.missing,
        cli.exclude.clone(),
        cli.no_default_excludes,
        cli.allow.clone(),
        cli.min,
        cli.top,
        cli.sort,
        cli.format,
        cli.summary,
        cli.fail_above,
        cli.fail_regression,
        cli.epsilon,
        cli.jobs,
    ))
}

fn analyze_sources(
    cli: &Cli,
    config: &EffectiveConfig,
) -> Result<(Analysis, Option<git_diff::GitDiff>)> {
    let Some(base) = &cli.diff_base else {
        let analysis = dispatch_analysis(&cli.path, config)?;
        ensure_any_parsed(&analysis)?;
        return Ok((analysis, None));
    };
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

fn warn_diagnostics(analysis: &Analysis) {
    for diagnostic in &analysis.diagnostics {
        eprintln!("warning: {}", diagnostic.message);
    }
}

fn merge_sources(
    analysis: Analysis,
    config: &EffectiveConfig,
    selected: bool,
) -> Result<poly_crap::MergeResult> {
    let coverage = parse_coverage_files(&config.coverage)?;
    let merged = if selected {
        merge_selected(analysis, &coverage, config.missing)
    } else {
        merge(analysis, &coverage, config.missing)
    };
    if merged.diagnostics.source_only_count > 0 || merged.diagnostics.coverage_only_count > 0 {
        eprintln!(
            "warning: coverage scope mismatch: {} source-only file(s), {} coverage-only file(s)",
            merged.diagnostics.source_only_count, merged.diagnostics.coverage_only_count
        );
    }
    Ok(merged)
}

fn filter_entries(entries: Vec<Entry>, config: &EffectiveConfig) -> Result<Vec<Entry>> {
    report::filter_entries(entries, &config.allow, config.min, config.top, config.sort)
}

fn render_report(
    cli: &Cli,
    config: &EffectiveConfig,
    collected: Collected,
) -> Result<(String, bool)> {
    if let Some(path) = &cli.baseline {
        render_delta_report(path, config, collected.entries, collected.diagnostics)
    } else {
        render_absolute_report(cli, config, collected)
    }
}

fn render_delta_report(
    path: &std::path::Path,
    config: &EffectiveConfig,
    entries: Vec<Entry>,
    diagnostics: ScopeDiagnostics,
) -> Result<(String, bool)> {
    let baseline_entries = filter_entries(baseline::load(path)?, config)?;
    let delta = baseline::compare(entries, &baseline_entries, config.epsilon, diagnostics);
    let failed = config.fail_regression && report::regression_failed(&delta);
    let rendered = report::render_delta(&delta, config.format, config.summary)?;
    Ok((rendered, failed))
}

fn render_absolute_report(
    cli: &Cli,
    config: &EffectiveConfig,
    collected: Collected,
) -> Result<(String, bool)> {
    let failed =
        config.fail_above && report::threshold_failed(&collected.entries, config.threshold);
    let selected_units = collected.entries.len();
    let mut rendered = report::render_absolute(
        collected.entries,
        collected.diagnostics,
        config.format,
        config.threshold,
        config.summary,
    )?;
    if config.format == OutputFormat::Human
        && let (Some(requested), Some(diff)) = (&cli.diff_base, collected.diff)
    {
        rendered = format!(
            "Git diff against {requested} from {}: {} changed file(s), {selected_units} selected unit(s).\n{rendered}",
            diff.merge_base,
            diff.files.len(),
        );
    }
    Ok((rendered, failed))
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
}
