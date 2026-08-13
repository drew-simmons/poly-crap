use crate::analysis::Analysis;
use crate::coverage::{CoverageMap, FileCoverage};
use crate::model::{Entry, MetricKind, MissingCoveragePolicy, ScopeDiagnostics};
use crate::score::crap;
use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

#[derive(Debug)]
pub struct MergeResult {
    pub entries: Vec<Entry>,
    pub diagnostics: ScopeDiagnostics,
}

pub fn merge(
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
    let mut entries = Vec::new();

    for unit in analysis.units {
        if unit.language == crate::model::Language::Terraform {
            entries.push(Entry {
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
            });
            continue;
        }

        let found = lookup_coverage(&unit.file, coverage);
        if let Some((path, _)) = found {
            used_coverage.insert(path.clone());
            matched_sources.insert(unit.file.clone());
        }
        let measured = found.and_then(|(_, file)| {
            file.coverage_in_span(unit.start_line, unit.end_line)
                .map(|value| (file, value))
        });
        let coverage_value = measured.map(|(_, value)| value);
        let score_coverage = match (coverage_value, policy) {
            (Some(value), _) => value,
            (None, MissingCoveragePolicy::Pessimistic) => 0.0,
            (None, MissingCoveragePolicy::Optimistic) => 100.0,
            (None, MissingCoveragePolicy::Skip) => continue,
        };
        let crap_score = crap(unit.complexity, score_coverage);
        entries.push(Entry {
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
        });
    }

    entries.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then(a.file.cmp(&b.file))
            .then(a.start_line.cmp(&b.start_line))
    });

    let mut source_only: Vec<_> = if has_coverage {
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
    let mut coverage_only: Vec<_> = coverage
        .keys()
        .filter(|path| !used_coverage.contains(*path))
        .cloned()
        .collect();
    source_only.sort();
    coverage_only.sort();
    let source_only_count = source_only.len();
    let coverage_only_count = coverage_only.len();
    source_only.truncate(10);
    coverage_only.truncate(10);

    MergeResult {
        entries,
        diagnostics: ScopeDiagnostics {
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
            warnings: analysis.diagnostics,
        },
    }
}

fn lookup_coverage<'a>(
    source: &Path,
    coverage: &'a CoverageMap,
) -> Option<(&'a PathBuf, &'a FileCoverage)> {
    if let Some(found) = coverage.get_key_value(source) {
        return Some(found);
    }

    if source.is_absolute()
        && let Ok(canonical_source) = source.canonicalize()
    {
        let exact: Vec<_> = coverage
            .iter()
            .filter(|(path, _)| path.is_absolute())
            .filter(|(path, _)| {
                path.canonicalize()
                    .is_ok_and(|value| value == canonical_source)
            })
            .collect();
        if exact.len() == 1 {
            return exact.into_iter().next();
        }
    }

    let mut best_score = 0;
    let mut best = None;
    let mut tied = false;
    for (path, file) in coverage {
        let score = common_suffix(source, path);
        if score > best_score {
            best_score = score;
            best = Some((path, file));
            tied = false;
        } else if score > 0 && score == best_score {
            tied = true;
        }
    }
    if best_score > 0 && !tied { best } else { None }
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
    }
}
