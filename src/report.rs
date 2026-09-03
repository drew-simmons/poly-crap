use crate::baseline::{DeltaReport, DeltaStatus};
use crate::model::{Entry, LineRange, ScopeDiagnostics};
use anyhow::{Context, Result};
use clap::ValueEnum;
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::io::Write;
use std::path::Path;

const REPORT_SCHEMA: &str =
    "https://raw.githubusercontent.com/drew-simmons/poly-crap/main/schemas/report-v1.json";
const DELTA_SCHEMA: &str =
    "https://raw.githubusercontent.com/drew-simmons/poly-crap/main/schemas/delta-v1.json";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum OutputFormat {
    #[default]
    Human,
    Json,
    Sarif,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum SortOrder {
    #[default]
    Crap,
    File,
}

/// Which rows a report prints. The gates never read this; see [`gate_entries`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RowFilter {
    /// Every entry. Machine formats default to this so a JSON report can
    /// serve as a baseline.
    All,
    /// Entries scoring at least this much, from `--min`.
    AtLeast(f64),
    /// Entries over the threshold. The human table defaults to this so a
    /// large repository prints its failures rather than every function.
    Failing(f64),
}

impl RowFilter {
    /// The filter for an absolute report: `--min` when given, otherwise
    /// failing rows for human output and every row for machine output.
    #[must_use]
    pub fn for_report(format: OutputFormat, min: Option<f64>, threshold: f64) -> Self {
        match (min, format) {
            (Some(minimum), _) => Self::AtLeast(minimum),
            (None, OutputFormat::Human) => Self::Failing(threshold),
            (None, _) => Self::All,
        }
    }

    /// The filter for a delta report: `--min` when given, otherwise every row.
    ///
    /// A delta already hides unchanged functions, and a regression below the
    /// threshold still deserves a row, so the threshold plays no part here.
    #[must_use]
    pub fn explicit(min: Option<f64>) -> Self {
        min.map_or(Self::All, Self::AtLeast)
    }

    fn keeps(self, score: f64) -> bool {
        match self {
            Self::All => true,
            Self::AtLeast(minimum) => score >= minimum,
            Self::Failing(threshold) => score > threshold,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ReportEnvelope {
    #[serde(rename = "$schema")]
    pub schema: String,
    pub version: String,
    pub entries: Vec<Entry>,
    pub diagnostics: ScopeDiagnostics,
}

#[derive(Debug, Serialize)]
struct DeltaEnvelope<'a> {
    #[serde(rename = "$schema")]
    schema: &'static str,
    version: &'static str,
    entries: &'a [crate::baseline::DeltaEntry],
    removed: &'a [crate::baseline::RemovedEntry],
    diagnostics: &'a ScopeDiagnostics,
}

/// Build the entry set the gates and the summary counts are measured against.
///
/// Only `--allow` removes entries here. `--min` and `--top` are display limits
/// and must not reach this set: dropping a row would let a function above the
/// threshold exit 0 under `--fail-above`. Apply them with
/// [`apply_display_limits`] once the gate has run.
pub fn gate_entries(
    entries: Vec<Entry>,
    allow: &[String],
    root: &Path,
    sort: SortOrder,
) -> Result<Vec<Entry>> {
    let mut entries = filter_allowed(entries, allow, root)?;
    sort_entries(&mut entries, sort);
    Ok(entries)
}

/// Trim an already-gated set down to the rows worth printing.
///
/// Never call this before a gate or before a baseline comparison; see
/// [`gate_entries`].
#[must_use]
pub fn apply_display_limits(
    mut entries: Vec<Entry>,
    filter: RowFilter,
    top: Option<usize>,
    sort: SortOrder,
) -> Vec<Entry> {
    entries.retain(|entry| filter.keeps(entry.score));
    apply_top(&mut entries, top);
    sort_entries(&mut entries, sort);
    entries
}

/// Apply the same display limits to a delta report, scoring by the current run.
///
/// Removed entries have no current score, so the limits leave them alone.
pub fn limit_delta(report: &mut DeltaReport, filter: RowFilter, top: Option<usize>) {
    report
        .entries
        .retain(|entry| filter.keeps(entry.current.score));
    if let Some(limit) = top {
        report
            .entries
            .sort_by(|a, b| b.current.score.total_cmp(&a.current.score));
        report.entries.truncate(limit);
    }
}

/// Drop entries suppressed by `--allow`, leaving the display limits alone.
///
/// Baseline comparisons use this on its own. `--min` and `--top` decide what
/// gets printed; applying them to the baseline would strip the very rows a
/// regressed function has to match against, so it would be reported as new
/// and slip past `--fail-regression`.
pub fn filter_allowed(
    mut entries: Vec<Entry>,
    allow: &[String],
    root: &Path,
) -> Result<Vec<Entry>> {
    let (names, paths) = build_allow_sets(allow)?;
    entries
        .retain(|entry| !names.is_match(&entry.symbol) && !matches_path(&paths, &entry.file, root));
    Ok(entries)
}

/// Match an allow glob against the reported path and its root-relative form.
///
/// Reported paths keep the `--path` prefix, so they read as `./src/a.rs` by
/// default or as an absolute path. A documented pattern such as `vendor/**`
/// only lines up once the analysis root is stripped, which is what `--exclude`
/// already matches against. Both forms are tried so patterns written for
/// either shape keep working.
fn matches_path(paths: &GlobSet, file: &Path, root: &Path) -> bool {
    if paths.is_match(normalize_path(file)) {
        return true;
    }
    file.strip_prefix(root)
        .is_ok_and(|relative| paths.is_match(normalize_path(relative)))
}

fn normalize_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn build_allow_sets(allow: &[String]) -> Result<(GlobSet, GlobSet)> {
    let mut name_builder = GlobSetBuilder::new();
    let mut path_builder = GlobSetBuilder::new();
    for pattern in allow {
        add_allow_pattern(pattern, &mut name_builder, &mut path_builder)?;
    }
    finish_allow_sets(name_builder, path_builder)
}

fn finish_allow_sets(
    name_builder: GlobSetBuilder,
    path_builder: GlobSetBuilder,
) -> Result<(GlobSet, GlobSet)> {
    let names = name_builder
        .build()
        .context("building symbol allow patterns")?;
    let paths = path_builder
        .build()
        .context("building path allow patterns")?;
    Ok((names, paths))
}

fn add_allow_pattern(
    pattern: &str,
    names: &mut GlobSetBuilder,
    paths: &mut GlobSetBuilder,
) -> Result<()> {
    let path_pattern = is_path_pattern(pattern);
    let glob = GlobBuilder::new(pattern)
        .literal_separator(path_pattern)
        .build()
        .with_context(|| format!("invalid allow pattern: {pattern}"))?;
    if path_pattern {
        paths.add(glob);
    } else {
        names.add(glob);
    }
    Ok(())
}

fn is_path_pattern(pattern: &str) -> bool {
    pattern.contains('/') || pattern.contains("**")
}

fn apply_top(entries: &mut Vec<Entry>, top: Option<usize>) {
    if let Some(limit) = top {
        sort_entries(entries, SortOrder::Crap);
        entries.truncate(limit);
    }
}

pub fn sort_entries(entries: &mut [Entry], sort: SortOrder) {
    match sort {
        SortOrder::Crap => entries.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then(a.file.cmp(&b.file))
                .then(a.start_line.cmp(&b.start_line))
        }),
        SortOrder::File => entries.sort_by(|a, b| {
            a.file
                .cmp(&b.file)
                .then(a.symbol.cmp(&b.symbol))
                .then(a.start_line.cmp(&b.start_line))
        }),
    }
}

/// Counts for the human summary line, measured before display limits apply.
///
/// Printing `entries.len()` instead would report "0 of 0" once `--min` or
/// `--top` trimmed the rows, hiding how much was actually scanned.
#[derive(Debug, Clone, Copy)]
pub struct Totals {
    failures: usize,
    total: usize,
}

impl Totals {
    #[must_use]
    pub fn new(entries: &[Entry], threshold: f64) -> Self {
        Self::from_scores(entries.iter().map(|entry| entry.score), threshold)
    }

    fn from_scores(scores: impl ExactSizeIterator<Item = f64>, threshold: f64) -> Self {
        Self {
            total: scores.len(),
            failures: scores.filter(|score| *score > threshold).count(),
        }
    }
}

/// Counts for the delta summary lines, measured before display limits apply.
///
/// Both baseline gates read these counts, so a row trimmed by `--min` or
/// `--top` still fails the run and still shows up in the summary.
#[derive(Debug, Clone, Copy)]
pub struct DeltaTotals {
    regressions: usize,
    scores: Totals,
}

impl DeltaTotals {
    #[must_use]
    pub fn new(report: &DeltaReport, threshold: f64) -> Self {
        Self {
            regressions: report
                .entries
                .iter()
                .filter(|entry| entry.status == DeltaStatus::Regressed)
                .count(),
            scores: Totals::from_scores(
                report.entries.iter().map(|entry| entry.current.score),
                threshold,
            ),
        }
    }

    /// True when a score rose from the baseline by more than epsilon.
    #[must_use]
    pub const fn regressed(&self) -> bool {
        self.regressions > 0
    }

    /// True when a current score is above the threshold.
    ///
    /// A new function has no baseline to rise from, so this is the only gate
    /// that can catch one.
    #[must_use]
    pub const fn exceeded(&self) -> bool {
        self.scores.failures > 0
    }
}

pub fn render_absolute(
    entries: Vec<Entry>,
    diagnostics: ScopeDiagnostics,
    format: OutputFormat,
    threshold: f64,
    summary: bool,
    totals: Totals,
) -> Result<String> {
    match format {
        OutputFormat::Human => Ok(render_human(
            &entries,
            &diagnostics,
            threshold,
            summary,
            totals,
        )),
        OutputFormat::Json => serde_json::to_string_pretty(&ReportEnvelope {
            schema: REPORT_SCHEMA.into(),
            version: env!("CARGO_PKG_VERSION").into(),
            entries,
            diagnostics,
        })
        .context("serializing JSON report"),
        OutputFormat::Sarif => render_sarif(&entries, threshold),
    }
}

pub fn render_delta(
    report: &DeltaReport,
    format: OutputFormat,
    threshold: f64,
    summary: bool,
    totals: DeltaTotals,
) -> Result<String> {
    match format {
        OutputFormat::Human => Ok(render_delta_human(report, threshold, summary, totals)),
        OutputFormat::Json => serde_json::to_string_pretty(&DeltaEnvelope {
            schema: DELTA_SCHEMA,
            version: env!("CARGO_PKG_VERSION"),
            entries: &report.entries,
            removed: &report.removed,
            diagnostics: &report.diagnostics,
        })
        .context("serializing delta JSON report"),
        OutputFormat::Sarif => anyhow::bail!("SARIF output cannot be combined with --baseline"),
    }
}

fn render_human(
    entries: &[Entry],
    diagnostics: &ScopeDiagnostics,
    threshold: f64,
    summary: bool,
    totals: Totals,
) -> String {
    let mut output = Vec::new();
    render_crap_section(&mut output, entries, threshold, summary, totals);
    append_scope_summary(&mut output, diagnostics);
    String::from_utf8(output).expect("human report is UTF-8")
}

fn render_crap_section(
    output: &mut Vec<u8>,
    entries: &[Entry],
    threshold: f64,
    summary: bool,
    totals: Totals,
) {
    writeln!(output, "CRAP results").unwrap();
    if !summary {
        render_crap_rows(output, entries, totals);
    }
    append_threshold_summary(output, totals, threshold);
}

fn render_crap_rows(output: &mut Vec<u8>, entries: &[Entry], totals: Totals) {
    writeln!(
        output,
        "  CRAP     CC  Coverage  Language    Symbol  Location  Uncovered"
    )
    .unwrap();
    for entry in entries {
        render_crap_entry(output, entry);
    }
    append_hidden_rows(output, totals.total.saturating_sub(entries.len()));
}

/// Say how many rows the display limits dropped, so a short table is not
/// mistaken for a short report.
fn append_hidden_rows(output: &mut Vec<u8>, hidden: usize) {
    if hidden > 0 {
        writeln!(
            output,
            "  {hidden} more function(s) not shown; adjust --min or --top to list them."
        )
        .unwrap();
    }
}

fn append_threshold_summary(output: &mut Vec<u8>, totals: Totals, threshold: f64) {
    writeln!(
        output,
        "{} of {} function(s) exceed CRAP threshold {threshold:.1}.",
        totals.failures, totals.total
    )
    .unwrap();
}

fn render_crap_entry(output: &mut Vec<u8>, entry: &Entry) {
    let coverage = entry
        .coverage
        .map_or_else(|| "N/A".into(), |value| format!("{value:.1}%"));
    writeln!(
        output,
        "  {:>7.1}  {:>5.1}  {:>8}  {:<10}  {}  {}:{}{}",
        entry.score,
        entry.complexity,
        coverage,
        entry.language,
        entry.symbol,
        entry.file.display(),
        entry.start_line,
        uncovered_cell(&entry.uncovered)
    )
    .unwrap();
}

/// Most uncovered ranges one row lists before it says how many more there are.
const MAX_RANGES: usize = 6;

/// The uncovered column with its separator, or nothing when there is no
/// uncovered line to point at.
fn uncovered_cell(ranges: &[LineRange]) -> String {
    if ranges.is_empty() {
        return String::new();
    }
    format!("  {}", format_ranges(ranges))
}

fn format_ranges(ranges: &[LineRange]) -> String {
    let mut listed: Vec<_> = ranges.iter().take(MAX_RANGES).map(format_range).collect();
    if ranges.len() > MAX_RANGES {
        listed.push(format!("+{} more", ranges.len() - MAX_RANGES));
    }
    listed.join(", ")
}

fn format_range(range: &LineRange) -> String {
    if range.start == range.end {
        range.start.to_string()
    } else {
        format!("{}-{}", range.start, range.end)
    }
}

fn render_delta_human(
    report: &DeltaReport,
    threshold: f64,
    summary: bool,
    totals: DeltaTotals,
) -> String {
    let mut output = Vec::new();
    writeln!(output, "Changes since baseline").unwrap();
    if !summary {
        render_delta_rows(&mut output, report);
    }
    writeln!(output, "{} regression(s) found.", totals.regressions).unwrap();
    append_threshold_summary(&mut output, totals.scores, threshold);
    append_scope_summary(&mut output, &report.diagnostics);
    String::from_utf8(output).expect("human report is UTF-8")
}

fn render_delta_rows(output: &mut Vec<u8>, report: &DeltaReport) {
    writeln!(
        output,
        "  Status     Score    Delta  Language    Symbol  Location  Uncovered"
    )
    .unwrap();
    for entry in report
        .entries
        .iter()
        .filter(|entry| entry.status != DeltaStatus::Unchanged)
    {
        render_delta_entry(output, entry);
    }
    for entry in &report.removed {
        writeln!(
            output,
            "  removed    {:>7.1}      N/A  {:<10}  {}  {}",
            entry.baseline_score,
            entry.language,
            entry.symbol,
            entry.file.display()
        )
        .unwrap();
    }
}

fn render_delta_entry(output: &mut Vec<u8>, entry: &crate::baseline::DeltaEntry) {
    let delta = entry
        .delta
        .map_or_else(|| "N/A".into(), |value| format!("{value:+.2}"));
    writeln!(
        output,
        "  {:<10} {:>7.1}  {:>7}  {:<10}  {}  {}:{}{}",
        format!("{:?}", entry.status).to_ascii_lowercase(),
        entry.current.score,
        delta,
        entry.current.language,
        entry.current.symbol,
        entry.current.file.display(),
        entry.current.start_line,
        uncovered_cell(&entry.current.uncovered)
    )
    .unwrap();
}

fn append_scope_summary(output: &mut Vec<u8>, diagnostics: &ScopeDiagnostics) {
    if diagnostics.coverage_files > 0 {
        writeln!(
            output,
            "Coverage scope: {} analyzed file(s), {} coverage source file(s), {} matched, {} source-only, {} coverage-only.",
            diagnostics.analyzed_files,
            diagnostics.coverage_files,
            diagnostics.matched_files,
            diagnostics.source_only_count,
            diagnostics.coverage_only_count
        )
        .unwrap();
    }
    if diagnostics.warning_count > 0 {
        writeln!(
            output,
            "{} source file warning(s).",
            diagnostics.warning_count
        )
        .unwrap();
    }
}

fn render_sarif(entries: &[Entry], threshold: f64) -> Result<String> {
    let results: Vec<_> = entries
        .iter()
        .filter(|entry| entry.score > threshold)
        .map(|entry| {
            json!({
                "ruleId": "poly-crap/crap-threshold",
                "level": "warning",
                "message": {"text": format!("{} has CRAP {:.1} (CC {:.1}, coverage {})", entry.symbol, entry.score, entry.complexity, entry.coverage.map_or_else(|| "N/A".into(), |v| format!("{v:.1}%")))},
                "locations": [{"physicalLocation": {
                    "artifactLocation": {"uri": normalize_path(&entry.file)},
                    "region": {"startLine": entry.start_line, "endLine": entry.end_line}
                }}]
            })
        })
        .collect();
    serde_json::to_string_pretty(&json!({
        "$schema": "https://json.schemastore.org/sarif-2.1.0.json",
        "version": "2.1.0",
        "runs": [{
            "tool": {"driver": {
                "name": "poly-crap",
                "version": env!("CARGO_PKG_VERSION"),
                "rules": [{
                    "id": "poly-crap/crap-threshold",
                    "shortDescription": {"text": "CRAP score exceeds the configured threshold"}
                }]
            }},
            "results": results
        }]
    }))
    .context("serializing SARIF report")
}

#[must_use]
pub fn threshold_failed(entries: &[Entry], threshold: f64) -> bool {
    entries.iter().any(|entry| entry.score > threshold)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::baseline::{DeltaEntry, RemovedEntry};
    use crate::model::{CoverageBasis, Language};
    use std::path::PathBuf;

    fn entry(score: f64) -> Entry {
        Entry {
            language: Language::Rust,
            file: PathBuf::from("src/a.rs"),
            symbol: "run".into(),
            start_line: 1,
            end_line: 2,
            complexity: 4.0,
            coverage: Some(50.0),
            coverage_basis: Some(CoverageBasis::Line),
            score,
            uncovered: Vec::new(),
        }
    }

    fn diagnostics() -> ScopeDiagnostics {
        ScopeDiagnostics {
            candidate_files: 2,
            parsed_files: 2,
            analyzed_files: 2,
            coverage_files: 1,
            matched_files: 1,
            source_only_count: 1,
            coverage_only_count: 0,
            warning_count: 1,
            source_only_examples: Vec::new(),
            coverage_only_examples: Vec::new(),
            warnings: Vec::new(),
        }
    }

    #[test]
    fn threshold_uses_the_crap_score() {
        assert!(!threshold_failed(&[entry(30.0)], 30.0));
        assert!(threshold_failed(&[entry(31.0)], 30.0));
    }

    #[test]
    fn top_keeps_the_highest_scores() {
        let filtered = apply_display_limits(
            vec![entry(4.0), entry(3.0), entry(2.0)],
            RowFilter::All,
            Some(2),
            SortOrder::Crap,
        );
        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].score, 4.0);
        assert_eq!(filtered[1].score, 3.0);
    }

    #[test]
    fn human_output_defaults_to_failing_rows() {
        // A human report lists failures; a JSON one keeps every entry so it
        // can serve as a baseline. `--min` overrides both.
        assert_eq!(
            RowFilter::for_report(OutputFormat::Human, None, 5.0),
            RowFilter::Failing(5.0)
        );
        assert_eq!(
            RowFilter::for_report(OutputFormat::Json, None, 5.0),
            RowFilter::All
        );
        assert_eq!(
            RowFilter::for_report(OutputFormat::Sarif, None, 5.0),
            RowFilter::All
        );
        assert_eq!(
            RowFilter::for_report(OutputFormat::Human, Some(0.0), 5.0),
            RowFilter::AtLeast(0.0)
        );
        assert_eq!(RowFilter::explicit(None), RowFilter::All);
        assert_eq!(RowFilter::explicit(Some(2.0)), RowFilter::AtLeast(2.0));
        // Failing is strict, like the gate. `--min` is inclusive, as documented.
        let entries = || vec![entry(6.0), entry(5.0), entry(3.0)];
        let failing =
            apply_display_limits(entries(), RowFilter::Failing(5.0), None, SortOrder::Crap);
        assert_eq!(failing.len(), 1);
        let at_least =
            apply_display_limits(entries(), RowFilter::AtLeast(5.0), None, SortOrder::Crap);
        assert_eq!(at_least.len(), 2);
    }

    #[test]
    fn human_output_names_hidden_rows_and_uncovered_lines() {
        let mut failing = entry(6.0);
        failing.uncovered = vec![
            LineRange { start: 5, end: 5 },
            LineRange { start: 8, end: 11 },
        ];
        let entries = vec![failing, entry(3.0)];
        let totals = Totals::new(&entries, 5.0);
        let shown = apply_display_limits(entries, RowFilter::Failing(5.0), None, SortOrder::Crap);
        let rendered = render_human(&shown, &diagnostics(), 5.0, false, totals);
        assert!(rendered.contains("Location  Uncovered"), "{rendered}");
        assert!(rendered.contains("src/a.rs:1  5, 8-11"), "{rendered}");
        assert!(
            rendered.contains("1 more function(s) not shown"),
            "{rendered}"
        );
        // A summary prints no rows, so it has nothing to say about hidden ones.
        let summary = render_human(&shown, &diagnostics(), 5.0, true, totals);
        assert!(!summary.contains("not shown"), "{summary}");
        // Nothing hidden, nothing said, and no trailing separator either.
        let all = vec![entry(6.0)];
        let full = render_human(&all, &diagnostics(), 5.0, false, Totals::new(&all, 5.0));
        assert!(!full.contains("not shown"), "{full}");
        assert!(full.contains("src/a.rs:1\n"), "{full}");
    }

    #[test]
    fn uncovered_ranges_are_listed_with_a_cap() {
        let single = |line| LineRange {
            start: line,
            end: line,
        };
        assert_eq!(format_ranges(&[]), "");
        assert_eq!(
            format_ranges(&[single(4), LineRange { start: 6, end: 9 }]),
            "4, 6-9"
        );
        let many: Vec<_> = (1..=8).map(|line| single(line * 2)).collect();
        assert_eq!(format_ranges(&many), "2, 4, 6, 8, 10, 12, +2 more");
        assert_eq!(uncovered_cell(&[]), "");
        assert_eq!(uncovered_cell(&[single(3)]), "  3");
    }

    #[test]
    fn display_limits_leave_the_gate_set_alone() {
        let entries = gate_entries(
            vec![entry(4.0), entry(3.0), entry(2.0)],
            &[],
            Path::new("."),
            SortOrder::Crap,
        )
        .unwrap();
        // `--min` must not decide the gate: every entry still has to reach
        // `threshold_failed`, and the summary must count all three.
        let totals = Totals::new(&entries, 1.0);
        assert_eq!((totals.failures, totals.total), (3, 3));
        assert!(threshold_failed(&entries, 1.0));
        assert!(
            apply_display_limits(entries, RowFilter::AtLeast(100.0), None, SortOrder::Crap)
                .is_empty()
        );
    }

    #[test]
    fn allow_patterns_filter_names_and_paths() {
        let mut generated = entry(4.0);
        generated.file = PathBuf::from("generated/a.rs");
        generated.symbol = "keep".into();
        let filtered = gate_entries(
            vec![entry(3.0), generated],
            &["run".into(), "generated/**".into()],
            Path::new("."),
            SortOrder::Crap,
        )
        .unwrap();
        assert!(filtered.is_empty());
        assert!(gate_entries(Vec::new(), &["[".into()], Path::new("."), SortOrder::Crap).is_err());
    }

    #[test]
    fn allow_paths_match_the_reported_and_root_relative_forms() {
        let prefixed = |file: &str| {
            let mut value = entry(3.0);
            value.file = PathBuf::from(file);
            value.symbol = "keep".into();
            value
        };
        // `--path .` reports `./vendor/a.rs`; `--path /repo` reports the
        // absolute path. The documented `vendor/**` must suppress both.
        for (root, file) in [(".", "./vendor/a.rs"), ("/repo", "/repo/vendor/a.rs")] {
            let filtered = gate_entries(
                vec![prefixed(file)],
                &["vendor/**".into()],
                Path::new(root),
                SortOrder::Crap,
            )
            .unwrap();
            assert!(filtered.is_empty(), "{file} survived under root {root}");
        }
    }

    #[test]
    fn human_output_reports_the_crap_section() {
        let entries = vec![entry(6.0), entry(3.0)];
        let totals = Totals::new(&entries, 5.0);
        let report = render_human(&entries, &diagnostics(), 5.0, false, totals);
        assert!(report.contains("CRAP results"));
        assert!(report.contains("1 of 2 function(s) exceed CRAP threshold 5.0."));
        assert!(
            render_human(&entries, &diagnostics(), 5.0, true, totals).contains("2 analyzed file")
        );
        // Trimmed rows must not shrink the counts the summary line reports.
        let shown = apply_display_limits(
            entries.clone(),
            RowFilter::AtLeast(5.5),
            None,
            SortOrder::Crap,
        );
        assert!(
            render_human(&shown, &diagnostics(), 5.0, false, totals)
                .contains("1 of 2 function(s) exceed CRAP threshold 5.0.")
        );
    }

    #[test]
    fn sarif_reports_only_entries_above_the_threshold() {
        let rendered = render_sarif(&[entry(6.0), entry(3.0)], 5.0).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&rendered).unwrap();
        assert_eq!(parsed["runs"][0]["results"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn delta_human_lists_changes_and_removals() {
        let report = DeltaReport {
            entries: vec![DeltaEntry {
                current: entry(8.0),
                baseline_score: Some(4.0),
                delta: Some(4.0),
                status: DeltaStatus::Regressed,
                previous_file: None,
            }],
            removed: vec![RemovedEntry {
                language: Language::Rust,
                file: PathBuf::from("src/old.rs"),
                symbol: "old".into(),
                baseline_score: 2.0,
            }],
            diagnostics: diagnostics(),
        };
        let totals = DeltaTotals::new(&report, 5.0);
        let rendered = render_delta_human(&report, 5.0, false, totals);
        assert!(rendered.contains("regressed"));
        assert!(rendered.contains("removed"));
        assert!(rendered.contains("1 of 1 function(s) exceed CRAP threshold 5.0."));
        assert!(render_delta_human(&report, 5.0, true, totals).contains("1 regression"));
    }

    #[test]
    fn delta_totals_are_counted_before_display_limits() {
        let mut report = DeltaReport {
            entries: vec![
                DeltaEntry {
                    current: entry(8.0),
                    baseline_score: Some(4.0),
                    delta: Some(4.0),
                    status: DeltaStatus::Regressed,
                    previous_file: None,
                },
                DeltaEntry {
                    current: entry(3.0),
                    baseline_score: None,
                    delta: None,
                    status: DeltaStatus::New,
                    previous_file: None,
                },
            ],
            removed: Vec::new(),
            diagnostics: diagnostics(),
        };
        // Neither gate may depend on which rows survive `--min` or `--top`.
        let totals = DeltaTotals::new(&report, 5.0);
        limit_delta(&mut report, RowFilter::AtLeast(100.0), None);
        assert!(report.entries.is_empty());
        assert!(totals.regressed());
        assert!(totals.exceeded());
        let trimmed = DeltaTotals::new(&report, 5.0);
        assert!(!trimmed.regressed());
        assert!(!trimmed.exceeded());
        let rendered = render_delta_human(&report, 5.0, false, totals);
        assert!(rendered.contains("1 regression(s) found."));
        assert!(rendered.contains("1 of 2 function(s) exceed CRAP threshold 5.0."));
    }
}
