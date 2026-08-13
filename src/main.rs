use anyhow::{Context, Result, bail};
use clap::{Parser, error::ErrorKind};
use poly_crap::baseline;
use poly_crap::config::{self, EffectiveConfig};
use poly_crap::model::{Language, MissingCoveragePolicy};
use poly_crap::report::{self, OutputFormat, SortOrder};
use poly_crap::{analyze_tree, merge, parse_coverage_files};
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
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) =>
        {
            print!("{error}");
            return ExitCode::SUCCESS;
        }
        Err(error) => {
            let _ = error.print();
            return ExitCode::from(2);
        }
    };
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
    let file_config = config::load(&cli.path)?;
    let effective = EffectiveConfig::new(
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
    );
    validate(&cli, &effective)?;

    let analysis = if let Some(jobs) = effective.jobs {
        rayon::ThreadPoolBuilder::new()
            .num_threads(jobs)
            .build()
            .context("creating source parsing thread pool")?
            .install(|| analyze_tree(&cli.path, &effective.languages, &effective.excludes))?
    } else {
        analyze_tree(&cli.path, &effective.languages, &effective.excludes)?
    };
    if analysis.candidate_files > 0 && analysis.parsed_files == 0 {
        bail!(
            "none of the {} candidate source files parsed successfully",
            analysis.candidate_files
        );
    }
    for diagnostic in &analysis.diagnostics {
        eprintln!("warning: {}", diagnostic.message);
    }

    let coverage = parse_coverage_files(&effective.coverage)?;
    let merged = merge(analysis, &coverage, effective.missing);
    if merged.diagnostics.source_only_count > 0 || merged.diagnostics.coverage_only_count > 0 {
        eprintln!(
            "warning: coverage scope mismatch: {} source-only file(s), {} coverage-only file(s)",
            merged.diagnostics.source_only_count, merged.diagnostics.coverage_only_count
        );
    }
    let entries = report::filter_entries(
        merged.entries,
        &effective.allow,
        effective.min,
        effective.top,
        effective.sort,
    )?;

    let (rendered, gate_failed) = if let Some(path) = &cli.baseline {
        let baseline_entries = report::filter_entries(
            baseline::load(path)?,
            &effective.allow,
            effective.min,
            effective.top,
            effective.sort,
        )?;
        let delta = baseline::compare(
            entries,
            &baseline_entries,
            effective.epsilon,
            merged.diagnostics,
        );
        let failed = effective.fail_regression && report::regression_failed(&delta);
        (
            report::render_delta(&delta, effective.format, effective.summary)?,
            failed,
        )
    } else {
        let failed =
            effective.fail_above && report::threshold_failed(&entries, effective.threshold);
        (
            report::render_absolute(
                entries,
                merged.diagnostics,
                effective.format,
                effective.threshold,
                effective.summary,
            )?,
            failed,
        )
    };
    write_report(&rendered, cli.output.as_ref())?;
    Ok(gate_failed)
}

fn validate(cli: &Cli, config: &EffectiveConfig) -> Result<()> {
    if !config.threshold.is_finite() || config.threshold < 0.0 {
        bail!("--threshold must be a finite non-negative number");
    }
    if !config.epsilon.is_finite() || config.epsilon < 0.0 {
        bail!("--epsilon must be a finite non-negative number");
    }
    if config.jobs == Some(0) {
        bail!("--jobs must be greater than zero");
    }
    if config.top == Some(0) {
        bail!("--top must be greater than zero");
    }
    if cli.baseline.is_some() && config.format == OutputFormat::Sarif {
        bail!("--baseline cannot be combined with --format sarif");
    }
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
}
