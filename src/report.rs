use crate::baseline::{DeltaReport, DeltaStatus};
use crate::model::{Entry, MetricKind, ScopeDiagnostics};
use anyhow::{Context, Result};
use clap::ValueEnum;
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::io::Write;

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

pub fn filter_entries(
    mut entries: Vec<Entry>,
    allow: &[String],
    min: Option<f64>,
    top: Option<usize>,
    sort: SortOrder,
) -> Result<Vec<Entry>> {
    let (names, paths) = build_allow_sets(allow)?;
    entries.retain(|entry| {
        !names.is_match(&entry.symbol)
            && !paths.is_match(entry.file.to_string_lossy().replace('\\', "/"))
            && min.is_none_or(|minimum| entry.score >= minimum)
    });
    apply_top(&mut entries, top);
    sort_entries(&mut entries, sort);
    Ok(entries)
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
        let mut seen = [0usize; 2];
        entries.retain(|entry| {
            let index = usize::from(entry.metric == MetricKind::Complexity);
            seen[index] += 1;
            seen[index] <= limit
        });
    }
}

pub fn sort_entries(entries: &mut [Entry], sort: SortOrder) {
    match sort {
        SortOrder::Crap => entries.sort_by(|a, b| {
            metric_rank(a.metric)
                .cmp(&metric_rank(b.metric))
                .then(b.score.total_cmp(&a.score))
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

pub fn render_absolute(
    entries: Vec<Entry>,
    diagnostics: ScopeDiagnostics,
    format: OutputFormat,
    threshold: f64,
    summary: bool,
) -> Result<String> {
    match format {
        OutputFormat::Human => Ok(render_human(&entries, &diagnostics, threshold, summary)),
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

pub fn render_delta(report: &DeltaReport, format: OutputFormat, summary: bool) -> Result<String> {
    match format {
        OutputFormat::Human => Ok(render_delta_human(report, summary)),
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
) -> String {
    let mut output = Vec::new();
    let crap_entries: Vec<_> = entries
        .iter()
        .filter(|entry| entry.metric == MetricKind::Crap)
        .collect();
    let terraform_entries: Vec<_> = entries
        .iter()
        .filter(|entry| entry.metric == MetricKind::Complexity)
        .collect();
    render_crap_section(&mut output, &crap_entries, threshold, summary);
    render_terraform_section(&mut output, &terraform_entries, summary);
    append_scope_summary(&mut output, diagnostics);
    String::from_utf8(output).expect("human report is UTF-8")
}

fn render_crap_section(output: &mut Vec<u8>, entries: &[&Entry], threshold: f64, summary: bool) {
    let failures = entries
        .iter()
        .filter(|entry| entry.score > threshold)
        .count();
    writeln!(output, "CRAP results").unwrap();
    if !summary {
        writeln!(
            output,
            "  CRAP     CC  Coverage  Language    Symbol  Location"
        )
        .unwrap();
        for entry in entries {
            render_crap_entry(output, entry);
        }
    }
    writeln!(
        output,
        "{} of {} function(s) exceed CRAP threshold {threshold:.1}.",
        failures,
        entries.len()
    )
    .unwrap();
}

fn render_crap_entry(output: &mut Vec<u8>, entry: &Entry) {
    let coverage = entry
        .coverage
        .map_or_else(|| "N/A".into(), |value| format!("{value:.1}%"));
    writeln!(
        output,
        "  {:>7.1}  {:>5.1}  {:>8}  {:<10}  {}  {}:{}",
        entry.score,
        entry.complexity,
        coverage,
        entry.language,
        entry.symbol,
        entry.file.display(),
        entry.start_line
    )
    .unwrap();
}

fn render_terraform_section(output: &mut Vec<u8>, entries: &[&Entry], summary: bool) {
    if !entries.is_empty() {
        writeln!(output, "\nTerraform complexity").unwrap();
        if !summary {
            writeln!(output, "  Complexity  Block  Location").unwrap();
            for entry in entries {
                render_terraform_entry(output, entry);
            }
        }
        writeln!(
            output,
            "{} Terraform block(s) analyzed; CRAP threshold does not apply.",
            entries.len()
        )
        .unwrap();
    }
}

fn render_terraform_entry(output: &mut Vec<u8>, entry: &Entry) {
    writeln!(
        output,
        "  {:>10.1}  {}  {}:{}",
        entry.score,
        entry.symbol,
        entry.file.display(),
        entry.start_line
    )
    .unwrap();
}

fn render_delta_human(report: &DeltaReport, summary: bool) -> String {
    let mut output = Vec::new();
    writeln!(output, "Changes since baseline").unwrap();
    if !summary {
        render_delta_rows(&mut output, report);
    }
    let regressions = report
        .entries
        .iter()
        .filter(|entry| entry.status == DeltaStatus::Regressed)
        .count();
    writeln!(output, "{regressions} regression(s) found.").unwrap();
    append_scope_summary(&mut output, &report.diagnostics);
    String::from_utf8(output).expect("human report is UTF-8")
}

fn render_delta_rows(output: &mut Vec<u8>, report: &DeltaReport) {
    writeln!(
        output,
        "  Status     Score    Delta  Language    Symbol  Location"
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
        "  {:<10} {:>7.1}  {:>7}  {:<10}  {}  {}:{}",
        format!("{:?}", entry.status).to_ascii_lowercase(),
        entry.current.score,
        delta,
        entry.current.language,
        entry.current.symbol,
        entry.current.file.display(),
        entry.current.start_line
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
        .filter(|entry| entry.metric == MetricKind::Crap && entry.score > threshold)
        .map(|entry| {
            json!({
                "ruleId": "poly-crap/crap-threshold",
                "level": "warning",
                "message": {"text": format!("{} has CRAP {:.1} (CC {:.1}, coverage {})", entry.symbol, entry.score, entry.complexity, entry.coverage.map_or_else(|| "N/A".into(), |v| format!("{v:.1}%")))},
                "locations": [{"physicalLocation": {
                    "artifactLocation": {"uri": entry.file.to_string_lossy().replace('\\', "/")},
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

fn metric_rank(metric: MetricKind) -> u8 {
    match metric {
        MetricKind::Crap => 0,
        MetricKind::Complexity => 1,
    }
}

#[must_use]
pub fn threshold_failed(entries: &[Entry], threshold: f64) -> bool {
    entries
        .iter()
        .any(|entry| entry.metric == MetricKind::Crap && entry.score > threshold)
}

#[must_use]
pub fn regression_failed(report: &DeltaReport) -> bool {
    report
        .entries
        .iter()
        .any(|entry| entry.status == DeltaStatus::Regressed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::baseline::{DeltaEntry, RemovedEntry};
    use crate::model::{CoverageBasis, Language};
    use std::path::PathBuf;

    fn entry(metric: MetricKind, score: f64) -> Entry {
        Entry {
            language: if metric == MetricKind::Crap {
                Language::Rust
            } else {
                Language::Terraform
            },
            file: PathBuf::from("src/a.rs"),
            symbol: "run".into(),
            start_line: 1,
            end_line: 2,
            metric,
            complexity: 4.0,
            coverage: Some(50.0),
            coverage_basis: Some(CoverageBasis::Line),
            crap: (metric == MetricKind::Crap).then_some(score),
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
    fn threshold_ignores_terraform() {
        assert!(!threshold_failed(
            &[entry(MetricKind::Complexity, 100.0)],
            30.0
        ));
        assert!(threshold_failed(&[entry(MetricKind::Crap, 31.0)], 30.0));
    }

    #[test]
    fn top_is_per_metric() {
        let filtered = filter_entries(
            vec![
                entry(MetricKind::Crap, 4.0),
                entry(MetricKind::Crap, 3.0),
                entry(MetricKind::Complexity, 2.0),
            ],
            &[],
            None,
            Some(1),
            SortOrder::Crap,
        )
        .unwrap();
        assert_eq!(filtered.len(), 2);
    }

    #[test]
    fn allow_patterns_filter_names_and_paths() {
        let mut generated = entry(MetricKind::Crap, 4.0);
        generated.file = PathBuf::from("generated/a.rs");
        generated.symbol = "keep".into();
        let filtered = filter_entries(
            vec![entry(MetricKind::Crap, 3.0), generated],
            &["run".into(), "generated/**".into()],
            None,
            None,
            SortOrder::Crap,
        )
        .unwrap();
        assert!(filtered.is_empty());
        assert!(filter_entries(Vec::new(), &["[".into()], None, None, SortOrder::Crap).is_err());
    }

    #[test]
    fn human_output_separates_metrics() {
        let entries = vec![
            entry(MetricKind::Crap, 6.0),
            entry(MetricKind::Complexity, 3.0),
        ];
        let report = render_human(&entries, &diagnostics(), 5.0, false);
        assert!(report.contains("CRAP results"));
        assert!(report.contains("Terraform complexity"));
        assert!(render_human(&entries, &diagnostics(), 5.0, true).contains("2 analyzed file"));
        assert!(
            !render_human(&entries[..1], &diagnostics(), 5.0, false)
                .contains("Terraform complexity")
        );
    }

    #[test]
    fn delta_human_lists_changes_and_removals() {
        let current = entry(MetricKind::Crap, 8.0);
        let report = DeltaReport {
            entries: vec![DeltaEntry {
                current,
                baseline_score: Some(4.0),
                delta: Some(4.0),
                status: DeltaStatus::Regressed,
                previous_file: None,
            }],
            removed: vec![RemovedEntry {
                language: Language::Rust,
                file: PathBuf::from("src/old.rs"),
                symbol: "old".into(),
                metric: MetricKind::Crap,
                baseline_score: 2.0,
            }],
            diagnostics: diagnostics(),
        };
        let rendered = render_delta_human(&report, false);
        assert!(rendered.contains("regressed"));
        assert!(rendered.contains("removed"));
        assert!(render_delta_human(&report, true).contains("1 regression"));
    }
}
