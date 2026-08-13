use crate::model::{CoverageBasis, LineRange};
use anyhow::{Context, Result, anyhow, bail};
use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};
use std::collections::HashMap;
use std::path::PathBuf;

pub type CoverageMap = HashMap<PathBuf, FileCoverage>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverageRegion {
    pub start_line: usize,
    pub end_line: usize,
    pub units: u64,
    pub covered: bool,
}

#[derive(Debug, Clone)]
pub struct FileCoverage {
    pub basis: CoverageBasis,
    pub regions: Vec<CoverageRegion>,
}

impl FileCoverage {
    fn new(basis: CoverageBasis) -> Self {
        Self {
            basis,
            regions: Vec::new(),
        }
    }

    fn add(&mut self, region: CoverageRegion) {
        if let Some(existing) = self.regions.iter_mut().find(|candidate| {
            candidate.start_line == region.start_line
                && candidate.end_line == region.end_line
                && candidate.units == region.units
        }) {
            existing.covered |= region.covered;
        } else {
            self.regions.push(region);
        }
    }

    #[must_use]
    pub fn coverage_in_span(&self, start_line: usize, end_line: usize) -> Option<f64> {
        let regions: Vec<_> = self
            .regions
            .iter()
            .filter(|region| region.start_line <= end_line && region.end_line >= start_line)
            .collect();
        let total: u64 = regions.iter().map(|region| region.units).sum();
        if total == 0 {
            return None;
        }
        let covered: u64 = regions
            .iter()
            .filter(|region| region.covered)
            .map(|region| region.units)
            .sum();
        Some(covered as f64 / total as f64 * 100.0)
    }

    #[must_use]
    pub fn uncovered_in_span(&self, start_line: usize, end_line: usize) -> Vec<LineRange> {
        let mut lines: Vec<_> = self
            .regions
            .iter()
            .filter(|region| {
                !region.covered && region.start_line <= end_line && region.end_line >= start_line
            })
            .flat_map(|region| region.start_line.max(start_line)..=region.end_line.min(end_line))
            .collect();
        lines.sort_unstable();
        lines.dedup();
        let mut ranges: Vec<LineRange> = Vec::new();
        for line in lines {
            if let Some(last) = ranges.last_mut()
                && line <= last.end + 1
            {
                last.end = line;
            } else {
                ranges.push(LineRange {
                    start: line,
                    end: line,
                });
            }
        }
        ranges
    }
}

pub fn parse_coverage_files(paths: &[PathBuf]) -> Result<CoverageMap> {
    let mut output = CoverageMap::new();
    for path in paths {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("reading coverage file {}", path.display()))?;
        let parsed = parse_coverage(&raw)
            .with_context(|| format!("parsing coverage file {}", path.display()))?;
        merge_maps(&mut output, parsed)?;
    }
    Ok(output)
}

fn parse_coverage(raw: &str) -> Result<CoverageMap> {
    let trimmed = raw.trim_start();
    if trimmed.starts_with("mode:") {
        parse_go(raw)
    } else if trimmed.starts_with('<') && raw.contains("<report") {
        parse_jacoco(raw)
    } else if raw.lines().any(|line| line.starts_with("SF:")) {
        parse_lcov(raw)
    } else {
        bail!("unknown coverage format; expected LCOV, Go cover profile, or JaCoCo XML")
    }
}

fn merge_maps(target: &mut CoverageMap, source: CoverageMap) -> Result<()> {
    for (path, file) in source {
        match target.get_mut(&path) {
            Some(existing) => {
                if existing.basis != file.basis {
                    bail!(
                        "coverage basis conflict for {}: {:?} and {:?}",
                        path.display(),
                        existing.basis,
                        file.basis
                    );
                }
                for region in file.regions {
                    existing.add(region);
                }
            }
            None => {
                target.insert(path, file);
            }
        }
    }
    Ok(())
}

fn parse_lcov(raw: &str) -> Result<CoverageMap> {
    let mut output = CoverageMap::new();
    let mut current: Option<PathBuf> = None;
    for line in raw.lines() {
        if let Some(path) = line.strip_prefix("SF:") {
            current = Some(portable_path(path.trim()));
        } else if let Some(data) = line.strip_prefix("DA:") {
            let path = current
                .as_ref()
                .ok_or_else(|| anyhow!("LCOV DA record appeared before SF record"))?;
            let mut values = data.split(',');
            let line = values
                .next()
                .ok_or_else(|| anyhow!("LCOV DA record has no line"))?
                .parse::<usize>()
                .context("invalid LCOV line number")?;
            let hits = values
                .next()
                .ok_or_else(|| anyhow!("LCOV DA record has no hit count"))?
                .parse::<i64>()
                .context("invalid LCOV hit count")?;
            output
                .entry(path.clone())
                .or_insert_with(|| FileCoverage::new(CoverageBasis::Line))
                .add(CoverageRegion {
                    start_line: line,
                    end_line: line,
                    units: 1,
                    covered: hits > 0,
                });
        } else if line == "end_of_record" {
            current = None;
        }
    }
    if output.is_empty() {
        bail!("LCOV report contains no DA records");
    }
    Ok(output)
}

fn parse_go(raw: &str) -> Result<CoverageMap> {
    let mut lines = raw.lines();
    let mode = lines.next().unwrap_or_default();
    if !matches!(mode, "mode: set" | "mode: count" | "mode: atomic") {
        bail!("unsupported Go coverage mode: {mode}");
    }
    let mut output = CoverageMap::new();
    for line in lines.filter(|line| !line.trim().is_empty()) {
        let mut fields = line.rsplitn(3, ' ');
        let hits = fields
            .next()
            .ok_or_else(|| anyhow!("Go coverage record has no hit count"))?
            .parse::<u64>()
            .context("invalid Go hit count")?;
        let units = fields
            .next()
            .ok_or_else(|| anyhow!("Go coverage record has no statement count"))?
            .parse::<u64>()
            .context("invalid Go statement count")?;
        let location = fields
            .next()
            .ok_or_else(|| anyhow!("Go coverage record has no source range"))?;
        let (path, range) = location
            .rsplit_once(':')
            .ok_or_else(|| anyhow!("Go coverage record has no source range"))?;
        let (start, end) = range
            .split_once(',')
            .ok_or_else(|| anyhow!("Go coverage record has an invalid source range"))?;
        let start_line = start
            .split_once('.')
            .map_or(start, |(line, _)| line)
            .parse::<usize>()
            .context("invalid Go start line")?;
        let end_line = end
            .split_once('.')
            .map_or(end, |(line, _)| line)
            .parse::<usize>()
            .context("invalid Go end line")?;
        output
            .entry(portable_path(path))
            .or_insert_with(|| FileCoverage::new(CoverageBasis::Statement))
            .add(CoverageRegion {
                start_line,
                end_line,
                units,
                covered: hits > 0,
            });
    }
    if output.is_empty() {
        bail!("Go coverage report contains no records");
    }
    Ok(output)
}

fn parse_jacoco(raw: &str) -> Result<CoverageMap> {
    let mut reader = Reader::from_str(raw);
    reader.config_mut().trim_text(true);
    let mut package = String::new();
    let mut source_file: Option<PathBuf> = None;
    let mut output = CoverageMap::new();
    loop {
        match reader.read_event().context("reading JaCoCo XML")? {
            Event::Start(event) if event.name().as_ref() == b"package" => {
                package = attribute(&event, b"name", &reader)?.unwrap_or_default();
            }
            Event::Start(event) if event.name().as_ref() == b"sourcefile" => {
                let name = attribute(&event, b"name", &reader)?
                    .ok_or_else(|| anyhow!("JaCoCo sourcefile has no name"))?;
                source_file = Some(if package.is_empty() {
                    portable_path(&name)
                } else {
                    portable_path(&package).join(name)
                });
            }
            Event::Empty(event) if event.name().as_ref() == b"line" => {
                let path = source_file
                    .as_ref()
                    .ok_or_else(|| anyhow!("JaCoCo line appeared outside sourcefile"))?;
                let line = required_number(&event, b"nr", &reader)? as usize;
                let missed = required_number(&event, b"mi", &reader)?;
                let covered = required_number(&event, b"ci", &reader)?;
                if missed + covered > 0 {
                    output
                        .entry(path.clone())
                        .or_insert_with(|| FileCoverage::new(CoverageBasis::Line))
                        .add(CoverageRegion {
                            start_line: line,
                            end_line: line,
                            units: 1,
                            covered: covered > 0,
                        });
                }
            }
            Event::End(event) if event.name().as_ref() == b"sourcefile" => source_file = None,
            Event::Eof => break,
            _ => {}
        }
    }
    if output.is_empty() {
        bail!("JaCoCo report contains no executable source lines");
    }
    Ok(output)
}

fn attribute(
    event: &BytesStart<'_>,
    name: &[u8],
    reader: &Reader<&[u8]>,
) -> Result<Option<String>> {
    for attribute in event.attributes() {
        let attribute = attribute.context("reading XML attribute")?;
        if attribute.key.as_ref() == name {
            return Ok(Some(
                attribute
                    .decode_and_unescape_value(reader.decoder())
                    .context("decoding XML attribute")?
                    .into_owned(),
            ));
        }
    }
    Ok(None)
}

fn required_number(event: &BytesStart<'_>, name: &[u8], reader: &Reader<&[u8]>) -> Result<u64> {
    attribute(event, name, reader)?
        .ok_or_else(|| anyhow!("JaCoCo line is missing {}", String::from_utf8_lossy(name)))?
        .parse::<u64>()
        .with_context(|| format!("invalid JaCoCo {}", String::from_utf8_lossy(name)))
}

fn portable_path(value: &str) -> PathBuf {
    PathBuf::from(value.replace('\\', "/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn parses_and_merges_lcov() {
        let coverage = parse_lcov("SF:src/a.rs\nDA:1,0\nDA:2,2\nend_of_record\n").unwrap();
        let file = &coverage[Path::new("src/a.rs")];
        assert_eq!(file.coverage_in_span(1, 2), Some(50.0));
        assert_eq!(
            file.uncovered_in_span(1, 2),
            vec![LineRange { start: 1, end: 1 }]
        );
    }

    #[test]
    fn repeated_reports_union_hits() {
        let mut coverage = parse_lcov("SF:a.js\nDA:1,0\nend_of_record\n").unwrap();
        let second = parse_lcov("SF:a.js\nDA:1,3\nend_of_record\n").unwrap();
        merge_maps(&mut coverage, second).unwrap();
        assert_eq!(
            coverage[Path::new("a.js")].coverage_in_span(1, 1),
            Some(100.0)
        );
    }

    #[test]
    fn parses_go_statement_weights() {
        let coverage = parse_go("mode: set\na.go:2.1,4.2 3 1\na.go:5.1,5.8 1 0\n").unwrap();
        assert_eq!(
            coverage[Path::new("a.go")].coverage_in_span(1, 8),
            Some(75.0)
        );
    }

    #[test]
    fn parses_jacoco_lines() {
        let xml = r#"<report name="x"><package name="a/b"><sourcefile name="C.java"><line nr="2" mi="1" ci="0"/><line nr="3" mi="0" ci="2"/></sourcefile></package></report>"#;
        let coverage = parse_jacoco(xml).unwrap();
        assert_eq!(
            coverage[Path::new("a/b/C.java")].coverage_in_span(1, 4),
            Some(50.0)
        );
    }

    #[test]
    fn rejects_unknown_format() {
        assert!(parse_coverage("not coverage").is_err());
    }
}
