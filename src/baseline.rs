use crate::model::{Entry, Language, ScopeDiagnostics};
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
            "parsing baseline {}; expected current poly-crap JSON output, so regenerate an older baseline",
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
    let (mut entries, mut matched) = match_exact(current, baseline, epsilon);
    match_suffixes(&mut entries, baseline, epsilon, &mut matched);
    match_moves(&mut entries, baseline, epsilon, &mut matched);
    let removed = removed_entries(baseline, &matched);
    DeltaReport {
        entries,
        removed,
        diagnostics,
    }
}

fn match_exact(
    current: Vec<Entry>,
    baseline: &[Entry],
    epsilon: f64,
) -> (Vec<DeltaEntry>, HashSet<usize>) {
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
    (entries, matched)
}

/// Match the rest by language, symbol, and path, ignoring position.
///
/// An edit above a function moves its start line, and a baseline written from
/// another root spells its path differently, so neither belongs in this key.
/// Keying on the line here would report every function below an added import
/// as `moved`. The paths still have to agree on the file name and its
/// directory, and exactly one baseline entry has to fit best, or the function
/// stays `new` for [`match_moves`] to consider.
fn match_suffixes(
    entries: &mut [DeltaEntry],
    baseline: &[Entry],
    epsilon: f64,
    matched: &mut HashSet<usize>,
) {
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
                    && candidate.symbol == delta.current.symbol
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
}

fn match_moves(
    entries: &mut [DeltaEntry],
    baseline: &[Entry],
    epsilon: f64,
    matched: &mut HashSet<usize>,
) {
    let current_counts = current_symbol_counts(entries);
    let baseline_counts = baseline_symbol_counts(baseline, matched);
    for delta in entries
        .iter_mut()
        .filter(|entry| entry.status == DeltaStatus::New)
    {
        let key = symbol_key(&delta.current);
        if !is_unique_move(&key, &current_counts, &baseline_counts) {
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
}

fn is_unique_move(
    key: &(Language, String),
    current: &HashMap<(Language, String), usize>,
    baseline: &HashMap<(Language, String), usize>,
) -> bool {
    current.get(key) == Some(&1) && baseline.get(key) == Some(&1)
}

fn current_symbol_counts(entries: &[DeltaEntry]) -> HashMap<(Language, String), usize> {
    let mut counts = HashMap::new();
    for delta in entries
        .iter()
        .filter(|entry| entry.status == DeltaStatus::New)
    {
        *counts.entry(symbol_key(&delta.current)).or_insert(0) += 1;
    }
    counts
}

fn baseline_symbol_counts(
    baseline: &[Entry],
    matched: &HashSet<usize>,
) -> HashMap<(Language, String), usize> {
    let mut counts = HashMap::new();
    for (index, entry) in baseline.iter().enumerate() {
        if !matched.contains(&index) {
            *counts.entry(symbol_key(entry)).or_insert(0) += 1;
        }
    }
    counts
}

fn removed_entries(baseline: &[Entry], matched: &HashSet<usize>) -> Vec<RemovedEntry> {
    baseline
        .iter()
        .enumerate()
        .filter(|(index, _)| !matched.contains(index))
        .map(|(_, entry)| RemovedEntry {
            language: entry.language,
            file: entry.file.clone(),
            symbol: entry.symbol.clone(),
            baseline_score: entry.score,
        })
        .collect()
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

fn exact_key(entry: &Entry) -> (Language, String, String, usize) {
    (
        entry.language,
        normalize_path(&entry.file),
        entry.symbol.clone(),
        entry.start_line,
    )
}

fn symbol_key(entry: &Entry) -> (Language, String) {
    (entry.language, entry.symbol.clone())
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
            complexity: score,
            coverage: Some(50.0),
            coverage_basis: Some(CoverageBasis::Line),
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
    fn detects_move_across_files() {
        let baseline = vec![entry(Language::Rust, "old/a.rs", "run", 2.0)];
        let report = compare(
            vec![entry(Language::Rust, "new/a.rs", "run", 2.0)],
            &baseline,
            DEFAULT_EPSILON,
            diagnostics(),
        );
        assert_eq!(report.entries[0].status, DeltaStatus::Moved);
        assert_eq!(
            report.entries[0].previous_file,
            Some(PathBuf::from("old/a.rs"))
        );
    }

    fn shifted(file: &str, score: f64, start_line: usize) -> Entry {
        let mut entry = entry(Language::Rust, file, "run", score);
        entry.start_line = start_line;
        entry.end_line = start_line + 1;
        entry
    }

    #[test]
    fn a_line_shift_in_the_same_file_is_not_a_move() {
        // An edit above the function moves its start line and nothing else.
        let baseline = vec![entry(Language::Rust, "src/a.rs", "run", 2.0)];
        let report = compare(
            vec![shifted("src/a.rs", 2.0, 5)],
            &baseline,
            DEFAULT_EPSILON,
            diagnostics(),
        );
        assert_eq!(report.entries[0].status, DeltaStatus::Unchanged);
        assert_eq!(report.entries[0].previous_file, None);
        assert!(report.removed.is_empty());

        // The match still carries the score, so a real change still shows.
        let report = compare(
            vec![shifted("src/a.rs", 6.0, 5)],
            &baseline,
            DEFAULT_EPSILON,
            diagnostics(),
        );
        assert_eq!(report.entries[0].status, DeltaStatus::Regressed);
        assert_eq!(report.entries[0].baseline_score, Some(2.0));
    }

    #[test]
    fn a_baseline_from_another_root_matches_across_a_line_shift() {
        let baseline = vec![entry(Language::Rust, "/repo/src/a.rs", "run", 2.0)];
        let report = compare(
            vec![shifted("./src/a.rs", 2.0, 9)],
            &baseline,
            DEFAULT_EPSILON,
            diagnostics(),
        );
        assert_eq!(report.entries[0].status, DeltaStatus::Unchanged);
    }

    #[test]
    fn two_shifted_namesakes_in_one_file_stay_unmatched() {
        // Rust modules can put two `run`s in one file. When both shift, neither
        // baseline entry is the obvious partner, so neither is guessed at.
        let baseline = vec![shifted("src/a.rs", 2.0, 1), shifted("src/a.rs", 2.0, 10)];
        let report = compare(
            vec![shifted("src/a.rs", 2.0, 3), shifted("src/a.rs", 2.0, 12)],
            &baseline,
            DEFAULT_EPSILON,
            diagnostics(),
        );
        assert!(
            report
                .entries
                .iter()
                .all(|entry| entry.status == DeltaStatus::New)
        );
        assert_eq!(report.removed.len(), 2);
    }
}
