use anyhow::{Context, Result, anyhow, bail};
use poly_crap::Analysis;
use poly_crap::model::LineRange;
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

#[derive(Debug)]
pub struct GitDiff {
    pub merge_base: String,
    pub files: BTreeMap<PathBuf, Vec<LineRange>>,
}

#[derive(Debug)]
struct TrackedChange {
    old_path: Option<PathBuf>,
    current_path: PathBuf,
}

impl GitDiff {
    pub fn selected_paths(&self) -> Vec<PathBuf> {
        self.files
            .iter()
            .filter(|(_, ranges)| !ranges.is_empty())
            .map(|(path, _)| path.clone())
            .collect()
    }

    pub fn retain_changed_units(&self, root: &Path, analysis: &mut Analysis) {
        analysis.units.retain(|unit| {
            let relative = unit.file.strip_prefix(root).unwrap_or(&unit.file);
            self.files.get(relative).is_some_and(|ranges| {
                ranges
                    .iter()
                    .any(|range| range.start <= unit.end_line && range.end >= unit.start_line)
            })
        });
    }
}

pub fn discover(root: &Path, requested_base: &str) -> Result<GitDiff> {
    if requested_base.starts_with('-') {
        bail!("Git diff base cannot start with '-': {requested_base}");
    }
    let canonical_root = root
        .canonicalize()
        .with_context(|| format!("resolving analysis path {}", root.display()))?;
    if !canonical_root.is_dir() {
        bail!("analysis path is not a directory: {}", root.display());
    }
    let repository = repository_root(&canonical_root)?;
    let relative_root = canonical_root.strip_prefix(&repository).map_err(|_| {
        anyhow!(
            "analysis path {} is outside Git repository {}",
            canonical_root.display(),
            repository.display()
        )
    })?;
    let base_commit = resolve_commit(&repository, requested_base)?;
    let merge_base = resolve_merge_base(&repository, &base_commit)?;
    let changes = tracked_changes(&repository, &merge_base, relative_root)?;
    let mut files = BTreeMap::new();
    for change in changes {
        let Some(relative) = scan_relative(&change.current_path, relative_root) else {
            continue;
        };
        let current = repository.join(&change.current_path);
        if !current.is_file() {
            continue;
        }
        let ranges = changed_ranges(&repository, &merge_base, &change)?;
        files.insert(relative, ranges);
    }
    for path in untracked_files(&repository, relative_root)? {
        let Some(relative) = scan_relative(&path, relative_root) else {
            continue;
        };
        if repository.join(&path).is_file() {
            files.insert(
                relative,
                vec![LineRange {
                    start: 1,
                    end: usize::MAX,
                }],
            );
        }
    }
    Ok(GitDiff { merge_base, files })
}

fn repository_root(root: &Path) -> Result<PathBuf> {
    let output = run_git(
        root,
        [OsStr::new("rev-parse"), OsStr::new("--show-toplevel")],
    )?;
    if !output.status.success() {
        bail!("path is not in a Git repository: {}", stderr(&output));
    }
    let path = path_from_bytes(trim_ascii(&output.stdout))?;
    path.canonicalize()
        .with_context(|| format!("resolving Git repository root {}", path.display()))
}

fn resolve_commit(repository: &Path, requested: &str) -> Result<String> {
    let revision = format!("{requested}^{{commit}}");
    let output = run_git(
        repository,
        [
            OsStr::new("rev-parse"),
            OsStr::new("--verify"),
            OsStr::new(&revision),
        ],
    )?;
    if !output.status.success() {
        bail!("invalid Git diff base '{requested}': {}", stderr(&output));
    }
    ascii_output(&output.stdout, "resolved Git revision")
}

fn resolve_merge_base(repository: &Path, base_commit: &str) -> Result<String> {
    let output = run_git(
        repository,
        [
            OsStr::new("merge-base"),
            OsStr::new(base_commit),
            OsStr::new("HEAD"),
        ],
    )?;
    if !output.status.success() {
        bail!("finding Git merge base: {}", stderr(&output));
    }
    ascii_output(&output.stdout, "Git merge base")
}

fn tracked_changes(
    repository: &Path,
    merge_base: &str,
    relative_root: &Path,
) -> Result<Vec<TrackedChange>> {
    let pathspec = pathspec(relative_root);
    let output = run_git(
        repository,
        [
            OsStr::new("diff"),
            OsStr::new("--name-status"),
            OsStr::new("-z"),
            OsStr::new("--find-renames"),
            OsStr::new("--no-ext-diff"),
            OsStr::new("--no-color"),
            OsStr::new(merge_base),
            OsStr::new("--"),
            pathspec.as_os_str(),
        ],
    )?;
    if !output.status.success() {
        bail!("reading Git changed files: {}", stderr(&output));
    }
    parse_name_status(&output.stdout)
}

fn parse_name_status(raw: &[u8]) -> Result<Vec<TrackedChange>> {
    let fields: Vec<_> = raw
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty())
        .collect();
    let mut changes = Vec::new();
    let mut index = 0;
    while index < fields.len() {
        let status = std::str::from_utf8(fields[index]).context("reading Git change status")?;
        index += 1;
        let kind = status
            .chars()
            .next()
            .ok_or_else(|| anyhow!("Git returned an empty change status"))?;
        if matches!(kind, 'R' | 'C') {
            let old = fields
                .get(index)
                .ok_or_else(|| anyhow!("Git omitted the old path for status {status}"))?;
            let current = fields
                .get(index + 1)
                .ok_or_else(|| anyhow!("Git omitted the new path for status {status}"))?;
            index += 2;
            changes.push(TrackedChange {
                old_path: Some(path_from_bytes(old)?),
                current_path: path_from_bytes(current)?,
            });
        } else {
            let path = fields
                .get(index)
                .ok_or_else(|| anyhow!("Git omitted the path for status {status}"))?;
            index += 1;
            if kind != 'D' {
                changes.push(TrackedChange {
                    old_path: None,
                    current_path: path_from_bytes(path)?,
                });
            }
        }
    }
    Ok(changes)
}

fn changed_ranges(
    repository: &Path,
    merge_base: &str,
    change: &TrackedChange,
) -> Result<Vec<LineRange>> {
    let mut command = git_command(repository);
    command.args([
        "diff",
        "--unified=0",
        "--find-renames",
        "--no-ext-diff",
        "--no-color",
        merge_base,
        "--",
    ]);
    if let Some(old) = &change.old_path {
        command.arg(old);
    }
    command.arg(&change.current_path);
    let output = run_command(command)?;
    if !output.status.success() {
        bail!(
            "reading Git diff for {}: {}",
            change.current_path.display(),
            stderr(&output)
        );
    }
    parse_hunks(&output.stdout)
}

fn parse_hunks(raw: &[u8]) -> Result<Vec<LineRange>> {
    raw.split(|byte| *byte == b'\n')
        .filter(|line| line.starts_with(b"@@ "))
        .map(parse_hunk)
        .collect()
}

fn parse_hunk(line: &[u8]) -> Result<LineRange> {
    let header = std::str::from_utf8(line).context("reading Git diff hunk")?;
    let current = header
        .split_ascii_whitespace()
        .find(|part| part.starts_with('+'))
        .ok_or_else(|| anyhow!("invalid Git diff hunk: {header}"))?;
    let range = current.trim_start_matches('+');
    let (start, count) = range
        .split_once(',')
        .map_or((range, "1"), |(start, count)| (start, count));
    let start = start
        .parse::<usize>()
        .with_context(|| format!("invalid Git diff hunk start: {header}"))?;
    let count = count
        .parse::<usize>()
        .with_context(|| format!("invalid Git diff hunk length: {header}"))?;
    if count == 0 {
        let anchor = start.max(1);
        return Ok(LineRange {
            start: anchor,
            end: anchor,
        });
    }
    Ok(LineRange {
        start,
        end: start.saturating_add(count - 1),
    })
}

fn untracked_files(repository: &Path, relative_root: &Path) -> Result<Vec<PathBuf>> {
    let pathspec = pathspec(relative_root);
    let output = run_git(
        repository,
        [
            OsStr::new("ls-files"),
            OsStr::new("--others"),
            OsStr::new("--exclude-standard"),
            OsStr::new("-z"),
            OsStr::new("--"),
            pathspec.as_os_str(),
        ],
    )?;
    if !output.status.success() {
        bail!("reading Git untracked files: {}", stderr(&output));
    }
    output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty())
        .map(path_from_bytes)
        .collect()
}

fn scan_relative(repository_path: &Path, relative_root: &Path) -> Option<PathBuf> {
    if relative_root.as_os_str().is_empty() {
        Some(repository_path.to_path_buf())
    } else {
        repository_path
            .strip_prefix(relative_root)
            .ok()
            .map(Path::to_path_buf)
    }
}

fn pathspec(relative_root: &Path) -> PathBuf {
    if relative_root.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        relative_root.to_path_buf()
    }
}

fn git_command(directory: &Path) -> Command {
    let mut command = Command::new("git");
    command.arg("-C").arg(directory);
    command
}

fn run_git<I, S>(directory: &Path, arguments: I) -> Result<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = git_command(directory);
    command.args(arguments);
    run_command(command)
}

fn run_command(mut command: Command) -> Result<Output> {
    command.output().map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            anyhow!("Git executable was not found")
        } else {
            anyhow!(error).context("running Git")
        }
    })
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).trim().to_string()
}

fn ascii_output(raw: &[u8], name: &str) -> Result<String> {
    std::str::from_utf8(trim_ascii(raw))
        .with_context(|| format!("reading {name}"))
        .map(ToString::to_string)
}

fn trim_ascii(raw: &[u8]) -> &[u8] {
    let mut end = raw.len();
    while end > 0 && raw[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    &raw[..end]
}

#[cfg(unix)]
fn path_from_bytes(raw: &[u8]) -> Result<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    Ok(PathBuf::from(OsString::from_vec(raw.to_vec())))
}

#[cfg(not(unix))]
fn path_from_bytes(raw: &[u8]) -> Result<PathBuf> {
    Ok(PathBuf::from(
        String::from_utf8(raw.to_vec()).context("reading Git path")?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_current_hunk_ranges_and_deletion_anchors() {
        let ranges = parse_hunks(b"@@ -1,2 +1,3 @@\n@@ -10,4 +11,0 @@\n").unwrap();
        assert_eq!(ranges[0], LineRange { start: 1, end: 3 });
        assert_eq!(ranges[1], LineRange { start: 11, end: 11 });
    }

    #[test]
    fn parses_rename_status() {
        let changes =
            parse_name_status(b"R100\0old name.py\0new name.py\0M\0src/lib.rs\0").unwrap();
        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0].old_path, Some(PathBuf::from("old name.py")));
        assert_eq!(changes[0].current_path, PathBuf::from("new name.py"));
        assert_eq!(changes[1].current_path, PathBuf::from("src/lib.rs"));
    }
}
