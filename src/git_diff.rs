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
    validate_requested_base(requested_base)?;
    discover_validated(root, requested_base)
}

fn discover_validated(root: &Path, requested_base: &str) -> Result<GitDiff> {
    let (repository, relative_root) = repository_scope(root)?;
    let base_commit = resolve_commit(&repository, requested_base)?;
    let merge_base = resolve_merge_base(&repository, &base_commit)?;
    let files = collect_changed_files(&repository, &merge_base, &relative_root)?;
    Ok(GitDiff { merge_base, files })
}

fn validate_requested_base(requested_base: &str) -> Result<()> {
    if requested_base.starts_with('-') {
        bail!("Git diff base cannot start with '-': {requested_base}");
    }
    Ok(())
}

fn repository_scope(root: &Path) -> Result<(PathBuf, PathBuf)> {
    let canonical_root = canonical_analysis_root(root)?;
    let repository = repository_root(&canonical_root)?;
    relative_repository_root(canonical_root, repository)
}

fn relative_repository_root(
    canonical_root: PathBuf,
    repository: PathBuf,
) -> Result<(PathBuf, PathBuf)> {
    let relative_root = canonical_root.strip_prefix(&repository).map_err(|_| {
        anyhow!(
            "analysis path {} is outside Git repository {}",
            canonical_root.display(),
            repository.display()
        )
    })?;
    Ok((repository, relative_root.to_path_buf()))
}

fn canonical_analysis_root(root: &Path) -> Result<PathBuf> {
    let canonical_root = root
        .canonicalize()
        .with_context(|| format!("resolving analysis path {}", root.display()))?;
    if !canonical_root.is_dir() {
        bail!("analysis path is not a directory: {}", root.display());
    }
    Ok(canonical_root)
}

fn collect_changed_files(
    repository: &Path,
    merge_base: &str,
    relative_root: &Path,
) -> Result<BTreeMap<PathBuf, Vec<LineRange>>> {
    let mut files = tracked_ranges(repository, merge_base, relative_root)?;
    for path in untracked_files(repository, relative_root)? {
        add_untracked_file(&mut files, repository, relative_root, &path);
    }
    Ok(files)
}

/// Changed line ranges for every tracked file, from a single `git diff`.
///
/// A patch names the file each hunk belongs to, so one invocation covers the
/// whole scan path instead of one process per changed file.
fn tracked_ranges(
    repository: &Path,
    merge_base: &str,
    relative_root: &Path,
) -> Result<BTreeMap<PathBuf, Vec<LineRange>>> {
    let pathspec = pathspec(relative_root);
    let output = run_git(
        repository,
        [
            // Keep non-ASCII paths as raw bytes rather than C-style escapes.
            OsStr::new("-c"),
            OsStr::new("core.quotePath=false"),
            OsStr::new("diff"),
            OsStr::new("--unified=0"),
            OsStr::new("--find-renames"),
            OsStr::new("--no-ext-diff"),
            OsStr::new("--no-color"),
            OsStr::new(merge_base),
            OsStr::new("--"),
            pathspec.as_os_str(),
        ],
    )?;
    if !output.status.success() {
        bail!("reading Git diff: {}", stderr(&output));
    }
    Ok(parse_patch(&output.stdout)?
        .into_iter()
        .filter(|(path, _)| repository.join(path).is_file())
        .filter_map(|(path, ranges)| Some((scan_relative(&path, relative_root)?, ranges)))
        .collect())
}

/// Position within one file's patch.
///
/// Under `--unified=0` every content line carries a `+` or `-` prefix, so a
/// file whose own contents look like a patch would otherwise have its
/// `+++ b/...` lines read as headers. Following the `diff --git`, `---`, `+++`
/// sequence keeps header lines and content lines apart.
#[derive(Debug, Clone, Copy, Default)]
enum PatchState {
    AwaitingOldPath,
    AwaitingNewPath,
    #[default]
    ReadingHunks,
}

#[derive(Debug, Default)]
struct PatchReader {
    files: BTreeMap<PathBuf, Vec<LineRange>>,
    current: Option<PathBuf>,
}

impl PatchReader {
    fn read_line(&mut self, state: PatchState, line: &[u8]) -> Result<PatchState> {
        if line.starts_with(b"diff --git ") {
            self.current = None;
            return Ok(PatchState::AwaitingOldPath);
        }
        self.read_body(state, line)
    }

    fn read_body(&mut self, state: PatchState, line: &[u8]) -> Result<PatchState> {
        match state {
            PatchState::AwaitingOldPath => Ok(await_old_path(line)),
            PatchState::AwaitingNewPath => self.read_new_path(line),
            PatchState::ReadingHunks => self.read_hunk(line).map(|()| state),
        }
    }

    fn read_new_path(&mut self, line: &[u8]) -> Result<PatchState> {
        if !line.starts_with(b"+++ ") {
            return Ok(PatchState::AwaitingNewPath);
        }
        self.current = patch_path(&line[4..])?;
        Ok(PatchState::ReadingHunks)
    }

    fn read_hunk(&mut self, line: &[u8]) -> Result<()> {
        let Some(path) = self.current.clone() else {
            return Ok(());
        };
        if line.starts_with(b"@@ ") {
            self.files.entry(path).or_default().push(parse_hunk(line)?);
        }
        Ok(())
    }
}

fn await_old_path(line: &[u8]) -> PatchState {
    if line.starts_with(b"--- ") {
        return PatchState::AwaitingNewPath;
    }
    PatchState::AwaitingOldPath
}

fn parse_patch(raw: &[u8]) -> Result<BTreeMap<PathBuf, Vec<LineRange>>> {
    let mut reader = PatchReader::default();
    let mut state = PatchState::default();
    for line in raw.split(|byte| *byte == b'\n') {
        state = reader.read_line(state, strip_carriage_return(line))?;
    }
    Ok(reader.files)
}

/// Path from a `+++` header, or `None` when there is no post-image to scan.
///
/// A deleted file reads `/dev/null`. A path holding a control character stays
/// quoted even with `core.quotePath=false`; such a file is skipped rather than
/// guessed at, so it simply does not enter the changed set.
fn patch_path(raw: &[u8]) -> Result<Option<PathBuf>> {
    if raw == b"/dev/null" || raw.starts_with(b"\"") {
        return Ok(None);
    }
    let trimmed = raw.strip_prefix(b"b/").unwrap_or(raw);
    path_from_bytes(trimmed).map(Some)
}

fn strip_carriage_return(line: &[u8]) -> &[u8] {
    line.strip_suffix(b"\r").unwrap_or(line)
}

fn add_untracked_file(
    files: &mut BTreeMap<PathBuf, Vec<LineRange>>,
    repository: &Path,
    relative_root: &Path,
    path: &Path,
) {
    let Some(relative) = scan_relative(path, relative_root) else {
        return;
    };
    if repository.join(path).is_file() {
        files.insert(
            relative,
            vec![LineRange {
                start: 1,
                end: usize::MAX,
            }],
        );
    }
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

fn parse_hunk(line: &[u8]) -> Result<LineRange> {
    let header = std::str::from_utf8(line).context("reading Git diff hunk")?;
    let range = header
        .split_ascii_whitespace()
        .find(|part| part.starts_with('+'))
        .ok_or_else(|| anyhow!("invalid Git diff hunk: {header}"))?
        .trim_start_matches('+');
    let (start, count) = parse_hunk_range(range, header)?;
    Ok(current_line_range(start, count))
}

fn parse_hunk_range(range: &str, header: &str) -> Result<(usize, usize)> {
    let (start, count) = range
        .split_once(',')
        .map_or((range, "1"), |(start, count)| (start, count));
    let start = start
        .parse::<usize>()
        .with_context(|| format!("invalid Git diff hunk start: {header}"))?;
    let count = count
        .parse::<usize>()
        .with_context(|| format!("invalid Git diff hunk length: {header}"))?;
    Ok((start, count))
}

fn current_line_range(start: usize, count: usize) -> LineRange {
    if count == 0 {
        let anchor = start.max(1);
        return LineRange {
            start: anchor,
            end: anchor,
        };
    }
    LineRange {
        start,
        end: start.saturating_add(count - 1),
    }
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

/// Environment variables that would override `-C <directory>`.
///
/// Git hooks and `git bisect run` set these, so an inherited value would
/// resolve revisions against a different repository than the one being
/// analysed, silently reporting a diff against an unrelated tree.
const OVERRIDING_GIT_VARIABLES: [&str; 5] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_COMMON_DIR",
    "GIT_OBJECT_DIRECTORY",
];

fn git_command(directory: &Path) -> Command {
    let mut command = Command::new("git");
    command.arg("-C").arg(directory);
    for variable in OVERRIDING_GIT_VARIABLES {
        command.env_remove(variable);
    }
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

    fn ranges(patch: &[u8], path: &str) -> Vec<LineRange> {
        parse_patch(patch)
            .unwrap()
            .remove(&PathBuf::from(path))
            .unwrap_or_default()
    }

    #[test]
    fn parses_current_hunk_ranges_and_deletion_anchors() {
        let patch = b"diff --git a/a.py b/a.py\n--- a/a.py\n+++ b/a.py\n@@ -1,2 +1,3 @@\n@@ -10,4 +11,0 @@\n";
        let found = ranges(patch, "a.py");
        assert_eq!(found[0], LineRange { start: 1, end: 3 });
        assert_eq!(found[1], LineRange { start: 11, end: 11 });
    }

    #[test]
    fn splits_hunks_across_files_and_keeps_renamed_paths() {
        let patch = b"diff --git a/old name.py b/new name.py\nsimilarity index 80%\nrename from old name.py\nrename to new name.py\n--- a/old name.py\n+++ b/new name.py\n@@ -1 +1,2 @@\ndiff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -7 +9,3 @@\n";
        let parsed = parse_patch(patch).unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(
            parsed[&PathBuf::from("new name.py")],
            vec![LineRange { start: 1, end: 2 }]
        );
        assert_eq!(
            parsed[&PathBuf::from("src/lib.rs")],
            vec![LineRange { start: 9, end: 11 }]
        );
    }

    #[test]
    fn ignores_patch_syntax_inside_changed_content() {
        // A file that itself contains a patch. Its added lines start with
        // `+++`/`@@` once prefixed, and must not be read as file headers.
        let patch = b"diff --git a/notes.py b/notes.py\n--- a/notes.py\n+++ b/notes.py\n@@ -1 +1,4 @@\n+diff --git a/evil.py b/evil.py\n+--- a/evil.py\n++++ b/evil.py\n+@@ -1 +900,5 @@\n";
        let parsed = parse_patch(patch).unwrap();
        assert_eq!(parsed.len(), 1, "content was parsed as a second file");
        assert_eq!(
            parsed[&PathBuf::from("notes.py")],
            vec![LineRange { start: 1, end: 4 }]
        );
    }

    #[test]
    fn waits_for_the_new_path_header() {
        // Git puts `+++` straight after `---`. Anything else leaves the reader
        // waiting rather than taking the line as a path.
        let patch =
            b"diff --git a/a.py b/a.py\n--- a/a.py\nunexpected\n+++ b/a.py\n@@ -1 +1,2 @@\n";
        assert_eq!(
            parse_patch(patch).unwrap()[&PathBuf::from("a.py")],
            vec![LineRange { start: 1, end: 2 }]
        );
    }

    #[test]
    fn skips_deleted_files_and_unreadable_paths() {
        let deleted =
            b"diff --git a/gone.py b/gone.py\n--- a/gone.py\n+++ /dev/null\n@@ -1,3 +0,0 @@\n";
        assert!(parse_patch(deleted).unwrap().is_empty());

        let quoted = b"diff --git \"a/od\\ny\" \"b/od\\ny\"\n--- \"a/od\\ny\"\n+++ \"b/od\\ny\"\n@@ -1 +1,2 @@\n";
        assert!(parse_patch(quoted).unwrap().is_empty());
    }
}
