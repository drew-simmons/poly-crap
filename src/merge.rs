use crate::analysis::Analysis;
use crate::coverage::{CoverageMap, FileCoverage};
use crate::model::{CodeUnit, Entry, MissingCoveragePolicy, ScopeDiagnostics};
use crate::score::crap;
use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

/// Scored functions plus the counts that say how well source and coverage
/// lined up.
#[derive(Debug)]
pub struct MergeResult {
    /// Every scored function, highest score first. Not yet gated or trimmed.
    pub entries: Vec<Entry>,
    pub diagnostics: ScopeDiagnostics,
}

/// Which coverage files the scope diagnostics should account for.
///
/// Matching always runs against the whole coverage map so that an ambiguous
/// path suffix stays ambiguous; only the reported scope narrows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CoverageScope {
    All,
    Matched,
}

/// Score every analyzed function against the coverage that matches its file.
///
/// A function whose file or lines have no coverage follows `policy`. Files in
/// the coverage map that no source matched count as coverage-only in the
/// diagnostics, which is what a whole-tree scan wants.
pub fn merge(
    analysis: Analysis,
    coverage: &CoverageMap,
    policy: MissingCoveragePolicy,
) -> MergeResult {
    merge_inner(analysis, coverage, policy, CoverageScope::All)
}

/// [`merge`] for an analysis that covered a subset of the tree, such as the
/// files a branch changed. Coverage for files outside the subset is expected
/// and is not reported as coverage-only.
pub fn merge_selected(
    analysis: Analysis,
    coverage: &CoverageMap,
    policy: MissingCoveragePolicy,
) -> MergeResult {
    merge_inner(analysis, coverage, policy, CoverageScope::Matched)
}

fn merge_inner(
    mut analysis: Analysis,
    coverage: &CoverageMap,
    policy: MissingCoveragePolicy,
    scope: CoverageScope,
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
            program_entry(
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
        scope,
    );
    MergeResult {
        entries,
        diagnostics: scope_diagnostics(
            &analysis,
            coverage_files(coverage, &used_coverage, scope),
            &analyzed_files,
            &matched_sources,
            source_only,
            coverage_only,
            warning_count,
        ),
    }
}

/// Coverage files in scope: every parsed file normally, and only the files the
/// selected sources matched when the run analysed a subset of the tree.
fn coverage_files(
    coverage: &CoverageMap,
    used_coverage: &HashSet<PathBuf>,
    scope: CoverageScope,
) -> usize {
    match scope {
        CoverageScope::All => coverage.len(),
        CoverageScope::Matched => used_coverage.len(),
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
        complexity: unit.complexity,
        coverage: coverage_value,
        coverage_basis: measured.map(|(file, _)| file.basis),
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
    scope: CoverageScope,
) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let source_only = if has_coverage {
        analyzed_files
            .iter()
            .filter(|path| {
                !matched_sources.contains(*path) && entries.iter().any(|entry| entry.file == **path)
            })
            .cloned()
            .collect()
    } else {
        Vec::new()
    };
    let coverage_only = if scope == CoverageScope::Matched {
        Vec::new()
    } else {
        coverage
            .keys()
            .filter(|path| !used_coverage.contains(*path))
            .cloned()
            .collect()
    };
    (source_only, coverage_only)
}

fn scope_diagnostics(
    analysis: &Analysis,
    coverage_files: usize,
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
        coverage_files,
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

/// Shortest suffix a match may rest on when the report keeps a directory.
const TRUSTED_SUFFIX: usize = 2;

/// The coverage entry sharing the longest trusted path suffix, if it is unique.
///
/// A match on the file name alone is only trusted when the report names the
/// file that way, as `SF:app.py` does. Otherwise `pkg_a/util.py` would borrow
/// the coverage of `pkg_b/util.py` whenever the tests never imported the
/// first, which is exactly the file the scan exists to flag. Reports that
/// keep a module prefix, such as Go's `github.com/org/repo/pkg/util.go`,
/// still match through the directory above the file.
fn suffix_match<'a>(
    source: &Path,
    coverage: &'a CoverageMap,
) -> Option<(&'a PathBuf, &'a FileCoverage)> {
    let source = normal_components(source);
    let scored: Vec<_> = coverage
        .iter()
        .filter_map(|(path, file)| suffix_score(&source, path).map(|score| (score, path, file)))
        .collect();
    let best = scored.iter().map(|(score, _, _)| *score).max()?;
    let mut matches = scored.iter().filter(|(score, _, _)| *score == best);
    let (_, path, file) = matches.next().copied()?;
    matches.next().is_none().then_some((path, file))
}

/// Trailing components `source` shares with a reported path, when enough to trust.
fn suffix_score(source: &[String], reported: &Path) -> Option<usize> {
    let reported = normal_components(reported);
    let shared = source
        .iter()
        .rev()
        .zip(reported.iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    trusted_suffix(shared, reported.len()).then_some(shared)
}

fn trusted_suffix(shared: usize, reported_len: usize) -> bool {
    shared >= TRUSTED_SUFFIX || (shared > 0 && shared == reported_len)
}

fn normal_components(path: &Path) -> Vec<String> {
    path.components().filter_map(normal_component).collect()
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
        assert_eq!(result.entries[0].score, 6.0);
        assert_eq!(result.diagnostics.matched_files, 1);
    }

    fn covered_file(path: &str) -> (PathBuf, FileCoverage) {
        (
            PathBuf::from(path),
            FileCoverage {
                basis: CoverageBasis::Line,
                regions: vec![CoverageRegion {
                    start_line: 1,
                    end_line: 1,
                    units: 1,
                    covered: true,
                }],
            },
        )
    }

    fn coverage_of(source: &str, reported: &str) -> Option<f64> {
        let coverage: CoverageMap = [covered_file(reported)].into_iter().collect();
        let mut analysis = analysis();
        analysis.units[0].file = PathBuf::from(source);
        merge(analysis, &coverage, MissingCoveragePolicy::Pessimistic).entries[0].coverage
    }

    #[test]
    fn a_shared_file_name_alone_does_not_borrow_coverage() {
        // The tests never imported `pkg_a`, so the report only knows `pkg_b`.
        // Matching on `util.py` would hand the untested file a clean score.
        assert_eq!(coverage_of("./pkg_a/util.py", "pkg_b/util.py"), None);
        // A report that names the file bare, as `SF:app.py` does, still matches.
        assert_eq!(coverage_of("./app.py", "app.py"), Some(100.0));
        assert_eq!(coverage_of("./sub/app.py", "app.py"), Some(100.0));
        // A module prefix still matches through the directory above the file.
        assert_eq!(
            coverage_of("/work/repo/pkg/util.go", "example.com/m/pkg/util.go"),
            Some(100.0)
        );
    }

    #[test]
    fn selected_runs_keep_ambiguous_coverage_unmatched() {
        // Two coverage files tie on the `lib/util.py` suffix, so neither is a
        // safe match. Narrowing the map for a selected run must not turn that
        // tie into a unique match against the wrong file.
        let mut coverage = HashMap::new();
        for name in ["x/lib/util.py", "y/lib/util.py"] {
            coverage.insert(
                PathBuf::from(name),
                FileCoverage {
                    basis: CoverageBasis::Line,
                    regions: vec![CoverageRegion {
                        start_line: 1,
                        end_line: 1,
                        units: 1,
                        covered: true,
                    }],
                },
            );
        }
        // `a/lib/util.py` ties between both coverage files. `x/lib/util.py`
        // matches one of them outright, which is what puts that file into the
        // scoped map and lets the tie resolve against the wrong source.
        let mut analysis = analysis();
        analysis.units[0].file = PathBuf::from("a/lib/util.py");
        let mut sibling = analysis.units[0].clone();
        sibling.file = PathBuf::from("x/lib/util.py");
        analysis.units.push(sibling);

        let full = merge(
            clone_analysis(&analysis),
            &coverage,
            MissingCoveragePolicy::Pessimistic,
        );
        let selected = merge_selected(analysis, &coverage, MissingCoveragePolicy::Pessimistic);
        assert_eq!(ambiguous_coverage(&full), None);
        assert_eq!(
            ambiguous_coverage(&selected),
            None,
            "selected run borrowed another file's coverage"
        );
    }

    fn ambiguous_coverage(result: &MergeResult) -> Option<f64> {
        result
            .entries
            .iter()
            .find(|entry| entry.file == PathBuf::from("a/lib/util.py"))
            .expect("ambiguous entry is present")
            .coverage
    }

    fn clone_analysis(analysis: &Analysis) -> Analysis {
        Analysis {
            units: analysis.units.clone(),
            candidate_files: analysis.candidate_files,
            parsed_files: analysis.parsed_files,
            diagnostics: analysis.diagnostics.clone(),
        }
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
