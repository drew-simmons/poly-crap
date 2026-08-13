use crate::model::{Entry, Language, MetricKind, ScopeDiagnostics};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub const DEFAULT_EPSILON: f64 = 0.01;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeltaStatus {
    New,
    Regressed,
    Improved,
    Unchanged,
    Moved,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeltaEntry {
    #[serde(flatten)]
    pub current: Entry,
    pub baseline_score: Option<f64>,
    pub delta: Option<f64>,
    pub status: DeltaStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_file: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemovedEntry {
    pub language: Language,
    pub file: PathBuf,
    pub symbol: String,
    pub metric: MetricKind,
    pub baseline_score: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeltaReport {
    pub entries: Vec<DeltaEntry>,
    pub removed: Vec<RemovedEntry>,
    pub diagnostics: ScopeDiagnostics,
}

#[derive(Deserialize)]
struct BaselineEnvelope {
    entries: Vec<Entry>,
}

pub fn load(path: &Path) -> Result<Vec<Entry>> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("reading baseline {}", path.display()))?;
    let envelope: BaselineEnvelope = serde_json::from_str(&raw).with_context(|| {
        format!(
            "parsing baseline {}; expected poly-crap JSON output",
            path.display()
        )
    })?;
    Ok(envelope.entries)
}

pub fn compare(
    current: Vec<Entry>,
    baseline: &[Entry],
    epsilon: f64,
    diagnostics: ScopeDiagnostics,
) -> DeltaReport {
    let exact: HashMap<_, _> = baseline
        .iter()
        .enumerate()
        .map(|(index, entry)| (exact_key(entry), index))
        .collect();
    let mut matched = HashSet::new();
    let mut entries = Vec::with_capacity(current.len());

    for entry in current {
        if let Some(index) = exact.get(&exact_key(&entry)).copied() {
            matched.insert(index);
            entries.push(delta_entry(
                entry,
                Some(&baseline[index]),
                epsilon,
                None,
                false,
            ));
        } else {
            entries.push(delta_entry(entry, None, epsilon, None, false));
        }
    }

    // Match a baseline recorded under a different checkout root.
    for delta in entries
        .iter_mut()
        .filter(|entry| entry.status == DeltaStatus::New)
    {
        let candidates: Vec<_> = baseline
            .iter()
            .enumerate()
            .filter(|(index, candidate)| {
                !matched.contains(index)
                    && candidate.language == delta.current.language
                    && candidate.metric == delta.current.metric
                    && candidate.symbol == delta.current.symbol
                    && candidate.start_line == delta.current.start_line
            })
            .map(|(index, candidate)| {
                (
                    index,
                    candidate,
                    suffix_len(&delta.current.file, &candidate.file),
                )
            })
            .filter(|(_, _, suffix)| *suffix > 1)
            .collect();
        let best = candidates
            .iter()
            .map(|(_, _, score)| *score)
            .max()
            .unwrap_or(0);
        let winners: Vec<_> = candidates
            .into_iter()
            .filter(|(_, _, score)| *score == best)
            .collect();
        if winners.len() == 1 {
            let (index, candidate, _) = winners[0];
            matched.insert(index);
            *delta = delta_entry(delta.current.clone(), Some(candidate), epsilon, None, false);
        }
    }

    // A unique language + symbol pair can move to a new file or line.
    let mut current_counts = HashMap::new();
    for delta in entries
        .iter()
        .filter(|entry| entry.status == DeltaStatus::New)
    {
        *current_counts
            .entry(symbol_key(&delta.current))
            .or_insert(0usize) += 1;
    }
    let mut baseline_counts = HashMap::new();
    for (index, entry) in baseline.iter().enumerate() {
        if !matched.contains(&index) {
            *baseline_counts.entry(symbol_key(entry)).or_insert(0usize) += 1;
        }
    }
    for delta in entries
        .iter_mut()
        .filter(|entry| entry.status == DeltaStatus::New)
    {
        let key = symbol_key(&delta.current);
        if current_counts.get(&key) != Some(&1) || baseline_counts.get(&key) != Some(&1) {
            continue;
        }
        if let Some((index, candidate)) = baseline
            .iter()
            .enumerate()
            .find(|(index, candidate)| !matched.contains(index) && symbol_key(candidate) == key)
        {
            matched.insert(index);
            let previous = Some(candidate.file.clone());
            *delta = delta_entry(
                delta.current.clone(),
                Some(candidate),
                epsilon,
                previous,
                true,
            );
        }
    }

    let removed = baseline
        .iter()
        .enumerate()
        .filter(|(index, _)| !matched.contains(index))
        .map(|(_, entry)| RemovedEntry {
            language: entry.language,
            file: entry.file.clone(),
            symbol: entry.symbol.clone(),
            metric: entry.metric,
            baseline_score: entry.score,
        })
        .collect();
    DeltaReport {
        entries,
        removed,
        diagnostics,
    }
}

fn delta_entry(
    current: Entry,
    baseline: Option<&Entry>,
    epsilon: f64,
    previous_file: Option<PathBuf>,
    moved: bool,
) -> DeltaEntry {
    let (baseline_score, delta, status) =
        baseline.map_or((None, None, DeltaStatus::New), |entry| {
            let change = current.score - entry.score;
            let status = if change > epsilon {
                DeltaStatus::Regressed
            } else if change < -epsilon {
                DeltaStatus::Improved
            } else if moved {
                DeltaStatus::Moved
            } else {
                DeltaStatus::Unchanged
            };
            (Some(entry.score), Some(change), status)
        });
    DeltaEntry {
        current,
        baseline_score,
        delta,
        status,
        previous_file,
    }
}

fn exact_key(entry: &Entry) -> (Language, MetricKind, String, String, usize) {
    (
        entry.language,
        entry.metric,
        normalize_path(&entry.file),
        entry.symbol.clone(),
        entry.start_line,
    )
}

fn symbol_key(entry: &Entry) -> (Language, MetricKind, String) {
    (entry.language, entry.metric, entry.symbol.clone())
}

fn normalize_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn suffix_len(left: &Path, right: &Path) -> usize {
    normalize_path(left)
        .split('/')
        .rev()
        .zip(normalize_path(right).split('/').rev())
        .take_while(|(left, right)| left == right)
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CoverageBasis, LineRange};

    fn entry(language: Language, file: &str, symbol: &str, score: f64) -> Entry {
        Entry {
            language,
            file: PathBuf::from(file),
            symbol: symbol.into(),
            start_line: 1,
            end_line: 2,
            metric: if language == Language::Terraform {
                MetricKind::Complexity
            } else {
                MetricKind::Crap
            },
            complexity: score,
            coverage: Some(50.0),
            coverage_basis: Some(CoverageBasis::Line),
            crap: Some(score),
            score,
            uncovered: Vec::<LineRange>::new(),
        }
    }

    fn diagnostics() -> ScopeDiagnostics {
        ScopeDiagnostics {
            candidate_files: 1,
            parsed_files: 1,
            analyzed_files: 1,
            coverage_files: 0,
            matched_files: 0,
            source_only_count: 0,
            coverage_only_count: 0,
            warning_count: 0,
            source_only_examples: Vec::new(),
            coverage_only_examples: Vec::new(),
            warnings: Vec::new(),
        }
    }

    #[test]
    fn detects_regression() {
        let baseline = vec![entry(Language::Rust, "src/a.rs", "run", 4.0)];
        let report = compare(
            vec![entry(Language::Rust, "src/a.rs", "run", 6.0)],
            &baseline,
            DEFAULT_EPSILON,
            diagnostics(),
        );
        assert_eq!(report.entries[0].status, DeltaStatus::Regressed);
    }

    #[test]
    fn detects_terraform_move() {
        let baseline = vec![entry(Language::Terraform, "old/a.tf", "resource.x", 2.0)];
        let report = compare(
            vec![entry(Language::Terraform, "new/a.tf", "resource.x", 2.0)],
            &baseline,
            DEFAULT_EPSILON,
            diagnostics(),
        );
        assert_eq!(report.entries[0].status, DeltaStatus::Moved);
    }
}
