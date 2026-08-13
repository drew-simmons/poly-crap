use crate::baseline::{DeltaReport, DeltaStatus};
use crate::model::{Entry, MetricKind, ScopeDiagnostics};
use anyhow::{Context, Result};
use clap::ValueEnum;
use globset::{GlobBuilder, GlobSetBuilder};
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
    let mut name_builder = GlobSetBuilder::new();
    let mut path_builder = GlobSetBuilder::new();
    for pattern in allow {
        let glob = GlobBuilder::new(pattern)
            .literal_separator(pattern.contains('/') || pattern.contains("**"))
            .build()
            .with_context(|| format!("invalid allow pattern: {pattern}"))?;
        if pattern.contains('/') || pattern.contains("**") {
            path_builder.add(glob);
        } else {
            name_builder.add(glob);
        }
    }
    let names = name_builder
        .build()
        .context("building symbol allow patterns")?;
    let paths = path_builder
        .build()
        .context("building path allow patterns")?;
    entries.retain(|entry| {
        !names.is_match(&entry.symbol)
            && !paths.is_match(entry.file.to_string_lossy().replace('\\', "/"))
            && min.is_none_or(|minimum| entry.score >= minimum)
    });
    if let Some(limit) = top {
        sort_entries(&mut entries, SortOrder::Crap);
        let mut seen = [0usize; 2];
        entries.retain(|entry| {
            let index = usize::from(entry.metric == MetricKind::Complexity);
            seen[index] += 1;
            seen[index] <= limit
        });
    }
    sort_entries(&mut entries, sort);
    Ok(entries)
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
    let failures = crap_entries
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
        for entry in crap_entries {
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
    }
    writeln!(
        output,
        "{} of {} function(s) exceed CRAP threshold {threshold:.1}.",
        failures,
        entries
            .iter()
            .filter(|entry| entry.metric == MetricKind::Crap)
            .count()
    )
    .unwrap();

    if !terraform_entries.is_empty() {
        writeln!(output, "\nTerraform complexity").unwrap();
        if !summary {
            writeln!(output, "  Complexity  Block  Location").unwrap();
            for entry in terraform_entries {
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
        }
        writeln!(
            output,
            "{} Terraform block(s) analyzed; CRAP threshold does not apply.",
            entries
                .iter()
                .filter(|entry| entry.metric == MetricKind::Complexity)
                .count()
        )
        .unwrap();
    }
    append_scope_summary(&mut output, diagnostics);
    String::from_utf8(output).expect("human report is UTF-8")
}

fn render_delta_human(report: &DeltaReport, summary: bool) -> String {
    let mut output = Vec::new();
    writeln!(output, "Changes since baseline").unwrap();
    if !summary {
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
    let regressions = report
        .entries
        .iter()
        .filter(|entry| entry.status == DeltaStatus::Regressed)
        .count();
    writeln!(output, "{regressions} regression(s) found.").unwrap();
    append_scope_summary(&mut output, &report.diagnostics);
    String::from_utf8(output).expect("human report is UTF-8")
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
}
