use crate::model::{CoverageBasis, LineRange};
use anyhow::{Context, Result, anyhow, bail};
use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, XmlVersion};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Coverage per file, keyed by the path exactly as the report wrote it.
/// Matching those paths to source files is the job of [`mod@crate::merge`].
pub type CoverageMap = HashMap<PathBuf, FileCoverage>;

/// Report locations coverage tools write to by default, relative to the scan
/// root and tried in this order. The skill wrapper's `COVERAGE_CANDIDATES` in
/// `.claude/skills/poly-crap/scripts/poly_crap.py` mirrors this list; change
/// both together.
pub const DEFAULT_REPORT_LOCATIONS: &[&str] = &[
    "coverage.lcov",
    "lcov.info",
    "coverage/lcov.info",
    "coverage/coverage.lcov",
    "coverage.out",
    "coverage.xml",
    "coverage/cobertura-coverage.xml",
    "target/site/jacoco/jacoco.xml",
    "build/reports/jacoco/test/jacocoTestReport.xml",
];

/// Every default-location report present under `root`, in list order.
#[must_use]
pub fn discover_reports(root: &Path) -> Vec<PathBuf> {
    DEFAULT_REPORT_LOCATIONS
        .iter()
        .map(|location| root.join(location))
        .filter(|path| path.is_file())
        .collect()
}
type CoverageDetector = fn(&str) -> bool;
type CoverageParser = fn(&str) -> Result<CoverageMap>;

const COVERAGE_PARSERS: [(CoverageDetector, CoverageParser); 4] = [
    (is_go_report, parse_go),
    (is_jacoco_report, parse_jacoco),
    (is_cobertura_report, parse_cobertura),
    (is_lcov_report, parse_lcov),
];

/// One measured stretch of a file: a single line for line-based reports, or a
/// statement block for Go, weighted by how many statements it holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverageRegion {
    pub start_line: usize,
    pub end_line: usize,
    pub units: u64,
    pub covered: bool,
}

/// Every measured region of one file, all on the same basis.
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

/// Read and merge coverage reports, detecting each one's format from its
/// contents. Where two reports measure the same region, a hit in either
/// counts. Two reports on different bases for one file is an error.
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
    COVERAGE_PARSERS
        .iter()
        .find(|(detect, _)| detect(raw))
        .map_or_else(unknown_format, |(_, parse)| parse(raw))
}

fn is_go_report(raw: &str) -> bool {
    raw.trim_start().starts_with("mode:")
}

fn is_jacoco_report(raw: &str) -> bool {
    raw.trim_start().starts_with('<') && raw.contains("<report")
}

fn is_cobertura_report(raw: &str) -> bool {
    raw.trim_start().starts_with('<') && raw.contains("<coverage")
}

fn is_lcov_report(raw: &str) -> bool {
    raw.lines().any(|line| line.starts_with("SF:"))
}

fn unknown_format() -> Result<CoverageMap> {
    bail!(
        "unknown coverage format; expected LCOV, Cobertura XML, JaCoCo XML, or a Go cover profile"
    )
}

fn merge_maps(target: &mut CoverageMap, source: CoverageMap) -> Result<()> {
    for (path, file) in source {
        merge_file(target, path, file)?;
    }
    Ok(())
}

fn merge_file(target: &mut CoverageMap, path: PathBuf, file: FileCoverage) -> Result<()> {
    let Some(existing) = target.get_mut(&path) else {
        target.insert(path, file);
        return Ok(());
    };
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
    Ok(())
}

fn parse_lcov(raw: &str) -> Result<CoverageMap> {
    let mut output = CoverageMap::new();
    let mut current: Option<PathBuf> = None;
    for line in raw.lines() {
        parse_lcov_record(line, &mut current, &mut output)?;
    }
    if output.is_empty() {
        bail!("LCOV report contains no DA records");
    }
    Ok(output)
}

fn parse_lcov_record(
    record: &str,
    current: &mut Option<PathBuf>,
    output: &mut CoverageMap,
) -> Result<()> {
    if let Some(path) = record.strip_prefix("SF:") {
        *current = Some(portable_path(path.trim()));
        return Ok(());
    }
    if let Some(data) = record.strip_prefix("DA:") {
        return add_lcov_data(data, current, output);
    }
    if record == "end_of_record" {
        *current = None;
    }
    Ok(())
}

fn add_lcov_data(data: &str, current: &Option<PathBuf>, output: &mut CoverageMap) -> Result<()> {
    let path = current
        .as_ref()
        .ok_or_else(|| anyhow!("LCOV DA record appeared before SF record"))?;
    let (line, hits) = parse_lcov_values(data)?;
    output
        .entry(path.clone())
        .or_insert_with(|| FileCoverage::new(CoverageBasis::Line))
        .add(CoverageRegion {
            start_line: line,
            end_line: line,
            units: 1,
            covered: hits > 0,
        });
    Ok(())
}

fn parse_lcov_values(data: &str) -> Result<(usize, i64)> {
    let mut values = data.split(',');
    let line = parse_lcov_line(values.next())?;
    let hits = parse_lcov_hits(values.next())?;
    Ok((line, hits))
}

fn parse_lcov_line(value: Option<&str>) -> Result<usize> {
    value
        .ok_or_else(|| anyhow!("LCOV DA record has no line"))
        .and_then(|value| value.parse().context("invalid LCOV line number"))
}

fn parse_lcov_hits(value: Option<&str>) -> Result<i64> {
    value
        .ok_or_else(|| anyhow!("LCOV DA record has no hit count"))
        .and_then(|value| value.parse().context("invalid LCOV hit count"))
}

fn parse_go(raw: &str) -> Result<CoverageMap> {
    let mut lines = raw.lines();
    let mode = lines.next().unwrap_or_default();
    validate_go_mode(mode)?;
    let mut output = CoverageMap::new();
    for line in lines.filter(|line| !line.trim().is_empty()) {
        add_go_record(line, &mut output)?;
    }
    finish_go_report(output)
}

fn validate_go_mode(mode: &str) -> Result<()> {
    if !matches!(mode, "mode: set" | "mode: count" | "mode: atomic") {
        bail!("unsupported Go coverage mode: {mode}");
    }
    Ok(())
}

fn finish_go_report(output: CoverageMap) -> Result<CoverageMap> {
    if output.is_empty() {
        bail!("Go coverage report contains no records");
    }
    Ok(output)
}

fn add_go_record(record: &str, output: &mut CoverageMap) -> Result<()> {
    let (location, units, hits) = parse_go_fields(record)?;
    let (path, start_line, end_line) = parse_go_location(location)?;
    output
        .entry(portable_path(path))
        .or_insert_with(|| FileCoverage::new(CoverageBasis::Statement))
        .add(CoverageRegion {
            start_line,
            end_line,
            units,
            covered: hits > 0,
        });
    Ok(())
}

fn parse_go_fields(record: &str) -> Result<(&str, u64, u64)> {
    let mut fields = record.rsplitn(3, ' ');
    let hits = parse_go_number(fields.next(), "hit count")?;
    let units = parse_go_number(fields.next(), "statement count")?;
    let location = fields
        .next()
        .ok_or_else(|| anyhow!("Go coverage record has no source range"))?;
    Ok((location, units, hits))
}

fn parse_go_number(value: Option<&str>, field: &str) -> Result<u64> {
    value
        .ok_or_else(|| anyhow!("Go coverage record has no {field}"))
        .and_then(|value| value.parse().with_context(|| format!("invalid Go {field}")))
}

fn parse_go_location(location: &str) -> Result<(&str, usize, usize)> {
    let (path, range) = location
        .rsplit_once(':')
        .ok_or_else(|| anyhow!("Go coverage record has no source range"))?;
    let (start, end) = range
        .split_once(',')
        .ok_or_else(|| anyhow!("Go coverage record has an invalid source range"))?;
    let start_line = parse_go_line(start, "start")?;
    let end_line = parse_go_line(end, "end")?;
    Ok((path, start_line, end_line))
}

fn parse_go_line(value: &str, position: &str) -> Result<usize> {
    let value = value.split_once('.').map_or(value, |(line, _)| line);
    value
        .parse()
        .with_context(|| format!("invalid Go {position} line"))
}

fn parse_jacoco(raw: &str) -> Result<CoverageMap> {
    let mut reader = Reader::from_str(raw);
    reader.config_mut().trim_text(true);
    let mut package = String::new();
    let mut source_file: Option<PathBuf> = None;
    let mut output = CoverageMap::new();
    while let Some(event) = read_xml_event(&mut reader, "JaCoCo")? {
        handle_jacoco_event(event, &reader, &mut package, &mut source_file, &mut output)?;
    }
    finish_jacoco_report(output)
}

fn finish_jacoco_report(output: CoverageMap) -> Result<CoverageMap> {
    if output.is_empty() {
        bail!("JaCoCo report contains no executable source lines");
    }
    Ok(output)
}

fn read_xml_event<'a>(reader: &mut Reader<&'a [u8]>, format: &str) -> Result<Option<Event<'a>>> {
    let event = reader
        .read_event()
        .with_context(|| format!("reading {format} XML"))?;
    Ok((event != Event::Eof).then_some(event))
}

fn handle_jacoco_event(
    event: Event<'_>,
    reader: &Reader<&[u8]>,
    package: &mut String,
    source_file: &mut Option<PathBuf>,
    output: &mut CoverageMap,
) -> Result<()> {
    match event {
        Event::Start(event) => handle_jacoco_start(&event, reader, package, source_file),
        Event::Empty(event) if event.name().as_ref() == b"line" => {
            add_jacoco_line(&event, reader, source_file, output)
        }
        Event::End(event) if event.name().as_ref() == b"sourcefile" => {
            *source_file = None;
            Ok(())
        }
        _ => Ok(()),
    }
}

fn handle_jacoco_start(
    event: &BytesStart<'_>,
    reader: &Reader<&[u8]>,
    package: &mut String,
    source_file: &mut Option<PathBuf>,
) -> Result<()> {
    if event.name().as_ref() == b"package" {
        *package = attribute(event, b"name", reader)?.unwrap_or_default();
    }
    if event.name().as_ref() == b"sourcefile" {
        *source_file = Some(jacoco_source_path(event, reader, package)?);
    }
    Ok(())
}

fn jacoco_source_path(
    event: &BytesStart<'_>,
    reader: &Reader<&[u8]>,
    package: &str,
) -> Result<PathBuf> {
    let name = attribute(event, b"name", reader)?
        .ok_or_else(|| anyhow!("JaCoCo sourcefile has no name"))?;
    if package.is_empty() {
        Ok(portable_path(&name))
    } else {
        Ok(portable_path(package).join(name))
    }
}

fn add_jacoco_line(
    event: &BytesStart<'_>,
    reader: &Reader<&[u8]>,
    source_file: &Option<PathBuf>,
    output: &mut CoverageMap,
) -> Result<()> {
    let path = source_file
        .as_ref()
        .ok_or_else(|| anyhow!("JaCoCo line appeared outside sourcefile"))?;
    let (line, missed, covered) = jacoco_line_numbers(event, reader)?;
    if missed + covered > 0 {
        output
            .entry(path.clone())
            .or_insert_with(|| FileCoverage::new(CoverageBasis::Line))
            .add(CoverageRegion {
                start_line: line as usize,
                end_line: line as usize,
                units: 1,
                covered: covered > 0,
            });
    }
    Ok(())
}

fn jacoco_line_numbers(event: &BytesStart<'_>, reader: &Reader<&[u8]>) -> Result<(u64, u64, u64)> {
    let line = required_number(event, b"nr", reader)?;
    let missed = required_number(event, b"mi", reader)?;
    let covered = required_number(event, b"ci", reader)?;
    Ok((line, missed, covered))
}

fn parse_cobertura(raw: &str) -> Result<CoverageMap> {
    let mut reader = Reader::from_str(raw);
    reader.config_mut().trim_text(true);
    let mut source_file: Option<PathBuf> = None;
    let mut output = CoverageMap::new();
    while let Some(event) = read_xml_event(&mut reader, "Cobertura")? {
        handle_cobertura_event(event, &reader, &mut source_file, &mut output)?;
    }
    finish_cobertura_report(output)
}

fn finish_cobertura_report(output: CoverageMap) -> Result<CoverageMap> {
    if output.is_empty() {
        bail!("Cobertura report contains no lines");
    }
    Ok(output)
}

/// A `<class filename="…">` names the file for every `<line>` under it. The
/// same lines appear under `<lines>` and again under each `<method>`; adding
/// a region twice collapses the repeat.
fn handle_cobertura_event(
    event: Event<'_>,
    reader: &Reader<&[u8]>,
    source_file: &mut Option<PathBuf>,
    output: &mut CoverageMap,
) -> Result<()> {
    match event {
        Event::Start(event) | Event::Empty(event) => {
            handle_cobertura_element(&event, reader, source_file, output)
        }
        Event::End(event) if event.name().as_ref() == b"class" => {
            *source_file = None;
            Ok(())
        }
        _ => Ok(()),
    }
}

fn handle_cobertura_element(
    event: &BytesStart<'_>,
    reader: &Reader<&[u8]>,
    source_file: &mut Option<PathBuf>,
    output: &mut CoverageMap,
) -> Result<()> {
    match event.name().as_ref() {
        b"class" => {
            *source_file = attribute(event, b"filename", reader)?.map(|name| portable_path(&name));
            Ok(())
        }
        b"line" => add_cobertura_line(event, reader, source_file, output),
        _ => Ok(()),
    }
}

fn add_cobertura_line(
    event: &BytesStart<'_>,
    reader: &Reader<&[u8]>,
    source_file: &Option<PathBuf>,
    output: &mut CoverageMap,
) -> Result<()> {
    let path = source_file
        .as_ref()
        .ok_or_else(|| anyhow!("Cobertura line appeared outside class"))?;
    let line = required_number(event, b"number", reader)?;
    let hits = required_number(event, b"hits", reader)?;
    output
        .entry(path.clone())
        .or_insert_with(|| FileCoverage::new(CoverageBasis::Line))
        .add(CoverageRegion {
            start_line: line as usize,
            end_line: line as usize,
            units: 1,
            covered: hits > 0,
        });
    Ok(())
}

fn attribute(
    event: &BytesStart<'_>,
    name: &[u8],
    reader: &Reader<&[u8]>,
) -> Result<Option<String>> {
    for attribute in event.attributes() {
        let attribute = attribute.context("reading XML attribute")?;
        if attribute.key.as_ref() == name {
            return decode_attribute(attribute, reader).map(Some);
        }
    }
    Ok(None)
}

fn decode_attribute(
    attribute: quick_xml::events::attributes::Attribute<'_>,
    reader: &Reader<&[u8]>,
) -> Result<String> {
    attribute
        .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
        .context("decoding XML attribute")
        .map(std::borrow::Cow::into_owned)
}

fn required_number(event: &BytesStart<'_>, name: &[u8], reader: &Reader<&[u8]>) -> Result<u64> {
    attribute(event, name, reader)?
        .ok_or_else(|| anyhow!("coverage line is missing {}", String::from_utf8_lossy(name)))?
        .parse::<u64>()
        .with_context(|| format!("invalid coverage line {}", String::from_utf8_lossy(name)))
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
        assert!(parse_go("mode: unknown\n").is_err());
        assert!(parse_go("mode: set\n").is_err());
    }

    #[test]
    fn parses_jacoco_lines() {
        let xml = r#"<report name="x"><package name="a/b"><sourcefile name="C.java"><line nr="2" mi="1" ci="0"/><line nr="3" mi="0" ci="2"/></sourcefile></package></report>"#;
        let coverage = parse_jacoco(xml).unwrap();
        assert_eq!(
            coverage[Path::new("a/b/C.java")].coverage_in_span(1, 4),
            Some(50.0)
        );
        assert!(parse_jacoco("<report name=\"empty\"></report>").is_err());
    }

    #[test]
    fn parses_cobertura_lines_once_per_file() {
        let xml = concat!(
            r#"<?xml version="1.0"?><coverage line-rate="0.5"><sources><source>/repo</source></sources>"#,
            r#"<packages><package name="pkg"><classes><class name="mod" filename="pkg/mod.py">"#,
            r#"<methods><method name="f"><lines><line number="2" hits="1"/></lines></method></methods>"#,
            r#"<lines><line number="2" hits="1"/>"#,
            r#"<line number="3" hits="0" branch="true" condition-coverage="50% (1/2)">"#,
            r#"<conditions><condition number="0" type="jump" coverage="50%"/></conditions></line>"#,
            r#"</lines></class></classes></package></packages></coverage>"#,
        );
        let coverage = parse_cobertura(xml).unwrap();
        let file = &coverage[Path::new("pkg/mod.py")];
        assert_eq!(
            file.regions.len(),
            2,
            "the method's copy of line 2 was counted twice"
        );
        assert_eq!(file.coverage_in_span(1, 4), Some(50.0));
        assert!(parse_cobertura("<coverage></coverage>").is_err());
    }

    #[test]
    fn detects_each_coverage_format() {
        assert!(parse_coverage("mode: set\na.go:1.1,1.2 1 1\n").is_ok());
        assert!(parse_coverage("SF:a.rs\nDA:1,1\nend_of_record\n").is_ok());
        assert!(
            parse_coverage(
                "<coverage><packages><package name=\"p\"><classes><class name=\"c\" filename=\"a.py\"><lines><line number=\"1\" hits=\"1\"/></lines></class></classes></package></packages></coverage>"
            )
            .is_ok()
        );
        assert!(
            parse_coverage(
                "<report><package name=\"\"><sourcefile name=\"A.java\"><line nr=\"1\" mi=\"0\" ci=\"1\"/></sourcefile></package></report>"
            )
            .is_ok()
        );
    }

    #[test]
    fn rejects_unknown_format() {
        assert!(parse_coverage("not coverage").is_err());
    }

    #[test]
    fn discovers_reports_at_default_locations_in_order() {
        let dir = tempfile::tempdir().unwrap();
        assert!(discover_reports(dir.path()).is_empty());
        std::fs::create_dir_all(dir.path().join("coverage")).unwrap();
        std::fs::write(dir.path().join("coverage/lcov.info"), "").unwrap();
        std::fs::write(dir.path().join("coverage.out"), "").unwrap();
        std::fs::write(dir.path().join("coverage.xml"), "").unwrap();
        assert_eq!(
            discover_reports(dir.path()),
            [
                dir.path().join("coverage/lcov.info"),
                dir.path().join("coverage.out"),
                dir.path().join("coverage.xml"),
            ]
        );
    }
}
