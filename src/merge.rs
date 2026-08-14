use crate::analysis::Analysis;
use crate::coverage::{CoverageMap, FileCoverage};
use crate::model::{
    CodeUnit, Entry, Language, MetricKind, MissingCoveragePolicy, ScopeDiagnostics,
};
use crate::score::crap;
use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

#[derive(Debug)]
pub struct MergeResult {
    pub entries: Vec<Entry>,
    pub diagnostics: ScopeDiagnostics,
}

pub fn merge(
    analysis: Analysis,
    coverage: &CoverageMap,
    policy: MissingCoveragePolicy,
) -> MergeResult {
    merge_inner(analysis, coverage, policy)
}

pub fn merge_selected(
    analysis: Analysis,
    coverage: &CoverageMap,
    policy: MissingCoveragePolicy,
) -> MergeResult {
    let scoped = scoped_coverage(&analysis, coverage);
    merge_inner(analysis, &scoped, policy)
}

fn merge_inner(
    mut analysis: Analysis,
    coverage: &CoverageMap,
    policy: MissingCoveragePolicy,
) -> MergeResult {
    let has_coverage = !coverage.is_empty();
    let warning_count = analysis.diagnostics.len();
    analysis.diagnostics.truncate(20);
    let mut used_coverage = HashSet::new();
    let mut matched_sources = HashSet::new();
    let analyzed_files: HashSet<_> = analysis
        .units
        .iter()
        .map(|unit| unit.file.clone())
        .collect();
    let units = std::mem::take(&mut analysis.units);
    let mut entries: Vec<_> = units
        .into_iter()
        .filter_map(|unit| {
            merge_unit(
                unit,
                coverage,
                policy,
                &mut used_coverage,
                &mut matched_sources,
            )
        })
        .collect();

    entries.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then(a.file.cmp(&b.file))
            .then(a.start_line.cmp(&b.start_line))
    });

    let (source_only, coverage_only) = scope_mismatches(
        has_coverage,
        &analyzed_files,
        &matched_sources,
        &used_coverage,
        coverage,
        &entries,
    );
    MergeResult {
        entries,
        diagnostics: scope_diagnostics(
            &analysis,
            coverage,
            &analyzed_files,
            &matched_sources,
            source_only,
            coverage_only,
            warning_count,
        ),
    }
}

fn scoped_coverage(analysis: &Analysis, coverage: &CoverageMap) -> CoverageMap {
    analysis
        .units
        .iter()
        .filter(|unit| unit.language != Language::Terraform)
        .filter_map(|unit| lookup_coverage(&unit.file, coverage))
        .map(|(path, file)| (path.clone(), file.clone()))
        .collect()
}

fn merge_unit(
    unit: CodeUnit,
    coverage: &CoverageMap,
    policy: MissingCoveragePolicy,
    used_coverage: &mut HashSet<PathBuf>,
    matched_sources: &mut HashSet<PathBuf>,
) -> Option<Entry> {
    if unit.language == Language::Terraform {
        return Some(terraform_entry(unit));
    }
    program_entry(unit, coverage, policy, used_coverage, matched_sources)
}

fn terraform_entry(unit: CodeUnit) -> Entry {
    Entry {
        language: unit.language,
        file: unit.file,
        symbol: unit.symbol,
        start_line: unit.start_line,
        end_line: unit.end_line,
        metric: MetricKind::Complexity,
        complexity: unit.complexity,
        coverage: None,
        coverage_basis: None,
        crap: None,
        score: unit.complexity,
        uncovered: Vec::new(),
    }
}

fn program_entry(
    unit: CodeUnit,
    coverage: &CoverageMap,
    policy: MissingCoveragePolicy,
    used_coverage: &mut HashSet<PathBuf>,
    matched_sources: &mut HashSet<PathBuf>,
) -> Option<Entry> {
    let found = lookup_coverage(&unit.file, coverage);
    record_match(&unit.file, found, used_coverage, matched_sources);
    let measured = found.and_then(|(_, file)| {
        file.coverage_in_span(unit.start_line, unit.end_line)
            .map(|value| (file, value))
    });
    let coverage_value = measured.map(|(_, value)| value);
    let score_coverage = coverage_for_score(coverage_value, policy)?;
    let crap_score = crap(unit.complexity, score_coverage);
    Some(Entry {
        language: unit.language,
        file: unit.file,
        symbol: unit.symbol,
        start_line: unit.start_line,
        end_line: unit.end_line,
        metric: MetricKind::Crap,
        complexity: unit.complexity,
        coverage: coverage_value,
        coverage_basis: measured.map(|(file, _)| file.basis),
        crap: Some(crap_score),
        score: crap_score,
        uncovered: measured.map_or_else(Vec::new, |(file, _)| {
            file.uncovered_in_span(unit.start_line, unit.end_line)
        }),
    })
}

fn record_match(
    source: &Path,
    found: Option<(&PathBuf, &FileCoverage)>,
    used_coverage: &mut HashSet<PathBuf>,
    matched_sources: &mut HashSet<PathBuf>,
) {
    if let Some((path, _)) = found {
        used_coverage.insert(path.clone());
        matched_sources.insert(source.to_path_buf());
    }
}

fn coverage_for_score(value: Option<f64>, policy: MissingCoveragePolicy) -> Option<f64> {
    if let Some(value) = value {
        return Some(value);
    }
    missing_coverage(policy)
}

fn missing_coverage(policy: MissingCoveragePolicy) -> Option<f64> {
    match policy {
        MissingCoveragePolicy::Pessimistic => Some(0.0),
        MissingCoveragePolicy::Optimistic => Some(100.0),
        MissingCoveragePolicy::Skip => None,
    }
}

fn scope_mismatches(
    has_coverage: bool,
    analyzed_files: &HashSet<PathBuf>,
    matched_sources: &HashSet<PathBuf>,
    used_coverage: &HashSet<PathBuf>,
    coverage: &CoverageMap,
    entries: &[Entry],
) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let source_only = if has_coverage {
        analyzed_files
            .iter()
            .filter(|path| {
                !matched_sources.contains(*path)
                    && entries
                        .iter()
                        .any(|entry| entry.file == **path && entry.metric == MetricKind::Crap)
            })
            .cloned()
            .collect()
    } else {
        Vec::new()
    };
    let coverage_only = coverage
        .keys()
        .filter(|path| !used_coverage.contains(*path))
        .cloned()
        .collect();
    (source_only, coverage_only)
}

fn scope_diagnostics(
    analysis: &Analysis,
    coverage: &CoverageMap,
    analyzed_files: &HashSet<PathBuf>,
    matched_sources: &HashSet<PathBuf>,
    mut source_only: Vec<PathBuf>,
    mut coverage_only: Vec<PathBuf>,
    warning_count: usize,
) -> ScopeDiagnostics {
    source_only.sort();
    coverage_only.sort();
    let source_only_count = source_only.len();
    let coverage_only_count = coverage_only.len();
    source_only.truncate(10);
    coverage_only.truncate(10);

    ScopeDiagnostics {
        candidate_files: analysis.candidate_files,
        parsed_files: analysis.parsed_files,
        analyzed_files: analyzed_files.len(),
        coverage_files: coverage.len(),
        matched_files: matched_sources.len(),
        source_only_count,
        coverage_only_count,
        warning_count,
        source_only_examples: source_only,
        coverage_only_examples: coverage_only,
        warnings: analysis.diagnostics.clone(),
    }
}

fn lookup_coverage<'a>(
    source: &Path,
    coverage: &'a CoverageMap,
) -> Option<(&'a PathBuf, &'a FileCoverage)> {
    if let Some(found) = coverage.get_key_value(source) {
        return Some(found);
    }
    canonical_match(source, coverage).or_else(|| suffix_match(source, coverage))
}

fn canonical_match<'a>(
    source: &Path,
    coverage: &'a CoverageMap,
) -> Option<(&'a PathBuf, &'a FileCoverage)> {
    if !source.is_absolute() {
        return None;
    }
    let canonical_source = source.canonicalize().ok()?;
    let mut exact = coverage
        .iter()
        .filter(|(path, _)| path.is_absolute())
        .filter(|(path, _)| {
            path.canonicalize()
                .is_ok_and(|value| value == canonical_source)
        });
    let found = exact.next()?;
    exact.next().is_none().then_some(found)
}

fn suffix_match<'a>(
    source: &Path,
    coverage: &'a CoverageMap,
) -> Option<(&'a PathBuf, &'a FileCoverage)> {
    let best_score = coverage
        .keys()
        .map(|path| common_suffix(source, path))
        .max()
        .unwrap_or(0);
    if best_score == 0 {
        return None;
    }
    let mut matches = coverage
        .iter()
        .filter(|(path, _)| common_suffix(source, path) == best_score);
    let found = matches.next()?;
    matches.next().is_none().then_some(found)
}

fn common_suffix(left: &Path, right: &Path) -> usize {
    let left: Vec<_> = left.components().filter_map(normal_component).collect();
    let right: Vec<_> = right.components().filter_map(normal_component).collect();
    left.iter()
        .rev()
        .zip(right.iter().rev())
        .take_while(|(a, b)| a == b)
        .count()
}

fn normal_component(component: Component<'_>) -> Option<String> {
    match component {
        Component::Normal(value) => Some(value.to_string_lossy().to_string()),
        Component::ParentDir => Some("..".into()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coverage::{CoverageRegion, FileCoverage};
    use crate::model::{CodeUnit, CoverageBasis, Diagnostic, Language};
    use std::collections::HashMap;

    fn analysis() -> Analysis {
        Analysis {
            units: vec![CodeUnit {
                language: Language::Rust,
                file: PathBuf::from("repo/src/lib.rs"),
                symbol: "run".into(),
                start_line: 1,
                end_line: 3,
                complexity: 4.0,
            }],
            candidate_files: 1,
            parsed_files: 1,
            diagnostics: Vec::<Diagnostic>::new(),
        }
    }

    #[test]
    fn suffix_matches_and_scores() {
        let mut coverage = HashMap::new();
        coverage.insert(
            PathBuf::from("src/lib.rs"),
            FileCoverage {
                basis: CoverageBasis::Line,
                regions: vec![
                    CoverageRegion {
                        start_line: 1,
                        end_line: 1,
                        units: 1,
                        covered: true,
                    },
                    CoverageRegion {
                        start_line: 2,
                        end_line: 2,
                        units: 1,
                        covered: false,
                    },
                ],
            },
        );
        let result = merge(analysis(), &coverage, MissingCoveragePolicy::Pessimistic);
        assert_eq!(result.entries[0].coverage, Some(50.0));
        assert_eq!(result.entries[0].crap, Some(6.0));
        assert_eq!(result.diagnostics.matched_files, 1);
    }

    #[test]
    fn missing_policy_can_skip() {
        let result = merge(analysis(), &HashMap::new(), MissingCoveragePolicy::Skip);
        assert!(result.entries.is_empty());

        let optimistic = merge(
            analysis(),
            &HashMap::new(),
            MissingCoveragePolicy::Optimistic,
        );
        assert_eq!(optimistic.entries[0].coverage, None);
        assert_eq!(optimistic.entries[0].score, 4.0);
    }
}
