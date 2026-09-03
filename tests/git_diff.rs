use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::*;
use serde_json::Value;
use std::fs;
use std::path::Path;
use std::process::Command;

fn write(path: &Path, value: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, value).unwrap();
}

fn git(directory: &Path, arguments: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(arguments)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {} failed: {}",
        arguments.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn init(directory: &Path) {
    git(directory, &["init", "-b", "main"]);
    git(directory, &["config", "user.email", "test@example.com"]);
    git(directory, &["config", "user.name", "Test User"]);
    git(directory, &["config", "commit.gpgsign", "false"]);
}

/// The binary with the color environment scrubbed, so a developer's
/// `CLICOLOR_FORCE` or `NO_COLOR` cannot change what a test sees.
fn poly_crap() -> assert_cmd::Command {
    let mut command = cargo_bin_cmd!("poly-crap");
    for name in ["NO_COLOR", "CLICOLOR", "CLICOLOR_FORCE"] {
        command.env_remove(name);
    }
    command
}

fn commit_all(directory: &Path, message: &str) {
    git(directory, &["add", "."]);
    git(directory, &["commit", "-m", message]);
}

fn json_report(directory: &Path, extra: &[&str]) -> Value {
    let mut command = poly_crap();
    command.args([
        "--path",
        directory.to_str().unwrap(),
        "--diff-base",
        "main",
        "--format",
        "json",
    ]);
    command.args(extra);
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn symbols(report: &Value) -> Vec<&str> {
    report["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["symbol"].as_str().unwrap())
        .collect()
}

#[test]
fn reports_only_the_changed_function_and_gates_it() {
    let dir = tempfile::tempdir().unwrap();
    init(dir.path());
    let source = dir.path().join("app.py");
    write(
        &source,
        "def edited(x):\n    return x\n\ndef untouched(a, b, c):\n    if a:\n        if b:\n            if c:\n                return 1\n    return 0\n",
    );
    commit_all(dir.path(), "base");
    git(dir.path(), &["checkout", "-b", "topic"]);
    write(
        &source,
        "def edited(a, b, c):\n    if a:\n        if b:\n            if c:\n                return 1\n    return 0\n\ndef untouched(a, b, c):\n    if a:\n        if b:\n            if c:\n                return 1\n    return 0\n",
    );

    let report = json_report(dir.path(), &[]);
    assert_eq!(symbols(&report), ["edited"]);
    let schema: Value = serde_json::from_str(include_str!("../schemas/report-v1.json")).unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    assert!(validator.is_valid(&report));

    poly_crap()
        .args([
            "--path",
            dir.path().to_str().unwrap(),
            "--diff-base",
            "main",
            "--threshold",
            "10",
            "--fail-above",
        ])
        .assert()
        .code(1)
        .stdout(predicate::str::contains(
            "1 changed file, 1 function selected.",
        ));

    let sarif = poly_crap()
        .args([
            "--path",
            dir.path().to_str().unwrap(),
            "--diff-base",
            "main",
            "--threshold",
            "10",
            "--format",
            "sarif",
        ])
        .output()
        .unwrap();
    assert!(sarif.status.success());
    let sarif: Value = serde_json::from_slice(&sarif.stdout).unwrap();
    assert_eq!(sarif["runs"][0]["results"].as_array().unwrap().len(), 1);
}

#[test]
fn includes_committed_staged_unstaged_and_untracked_work() {
    let dir = tempfile::tempdir().unwrap();
    init(dir.path());
    write(&dir.path().join(".gitignore"), "ignored.py\n");
    for name in ["committed", "staged", "unstaged", "unchanged", "excluded"] {
        write(
            &dir.path().join(format!("{name}.py")),
            &format!("def {name}(x):\n    return x\n"),
        );
    }
    write(
        &dir.path().join("日本語.py"),
        "def unicode_named(x):\n    return x\n",
    );
    commit_all(dir.path(), "base");
    git(dir.path(), &["checkout", "-b", "topic"]);

    write(
        &dir.path().join("committed.py"),
        "def committed(x):\n    if x:\n        return x\n    return 0\n",
    );
    commit_all(dir.path(), "topic change");
    write(
        &dir.path().join("staged.py"),
        "def staged(x):\n    if x:\n        return x\n    return 0\n",
    );
    git(dir.path(), &["add", "staged.py"]);
    write(
        &dir.path().join("unstaged.py"),
        "def unstaged(x):\n    if x:\n        return x\n    return 0\n",
    );
    write(
        &dir.path().join("untracked.py"),
        "def untracked(x):\n    if x:\n        return x\n    return 0\n",
    );
    write(
        &dir.path().join("space name.py"),
        "def spaced(x):\n    if x:\n        return x\n    return 0\n",
    );
    // Tracked, so its range comes from the patch. CJK has no NFD form, so the
    // name survives macOS filename normalisation unchanged.
    write(
        &dir.path().join("日本語.py"),
        "def unicode_named(x):\n    if x:\n        return x\n    return 0\n",
    );
    write(
        &dir.path().join("excluded.py"),
        "def excluded(x):\n    if x:\n        return x\n    return 0\n",
    );
    write(
        &dir.path().join("ignored.py"),
        "def ignored(x):\n    if x:\n        return x\n    return 0\n",
    );

    let coverage = dir.path().join("coverage.lcov");
    let mut lcov = String::new();
    for name in [
        "committed",
        "staged",
        "unstaged",
        "untracked",
        "unchanged",
        "excluded",
    ] {
        lcov.push_str(&format!(
            "SF:{}\nDA:1,1\nDA:2,0\nDA:3,1\nDA:4,1\nend_of_record\n",
            dir.path().join(format!("{name}.py")).display()
        ));
    }
    for name in ["space name.py", "日本語.py"] {
        lcov.push_str(&format!(
            "SF:{}\nDA:1,1\nDA:2,0\nDA:3,1\nDA:4,1\nend_of_record\n",
            dir.path().join(name).display()
        ));
    }
    write(&coverage, &lcov);

    let report = json_report(
        dir.path(),
        &[
            "--coverage",
            coverage.to_str().unwrap(),
            "--exclude",
            "excluded.py",
        ],
    );
    let mut found = symbols(&report);
    found.sort_unstable();
    assert_eq!(
        found,
        [
            "committed",
            "spaced",
            "staged",
            "unicode_named",
            "unstaged",
            "untracked"
        ]
    );
    assert_eq!(report["diagnostics"]["coverage_files"], 6);
    assert_eq!(report["diagnostics"]["matched_files"], 6);
    assert_eq!(report["diagnostics"]["coverage_only_count"], 0);
}

#[test]
fn uses_the_merge_base_instead_of_the_tip_of_main() {
    let dir = tempfile::tempdir().unwrap();
    init(dir.path());
    write(
        &dir.path().join("main_only.py"),
        "def main_only(x):\n    return x\n",
    );
    write(
        &dir.path().join("topic.py"),
        "def topic(x):\n    return x\n",
    );
    commit_all(dir.path(), "base");
    git(dir.path(), &["branch", "topic"]);

    write(
        &dir.path().join("main_only.py"),
        "def main_only(x):\n    if x:\n        return x\n    return 0\n",
    );
    commit_all(dir.path(), "main change");
    git(dir.path(), &["checkout", "topic"]);
    write(
        &dir.path().join("topic.py"),
        "def topic(x):\n    if x:\n        return x\n    return 0\n",
    );

    let report = json_report(dir.path(), &[]);
    assert_eq!(symbols(&report), ["topic"]);
}

#[test]
fn handles_renames_deletions_and_binary_files() {
    let dir = tempfile::tempdir().unwrap();
    init(dir.path());
    write(
        &dir.path().join("rename.py"),
        "def renamed(x):\n    return x\n",
    );
    write(&dir.path().join("pure.py"), "def pure(x):\n    return x\n");
    write(
        &dir.path().join("deleted.py"),
        "def deleted(x):\n    return x\n",
    );
    write(
        &dir.path().join("trimmed.py"),
        "def trimmed(x):\n    if x:\n        return x\n    return 0\n",
    );
    fs::write(dir.path().join("binary.py"), b"def binary():\0one\n").unwrap();
    commit_all(dir.path(), "base");
    git(dir.path(), &["checkout", "-b", "topic"]);

    git(dir.path(), &["mv", "rename.py", "renamed.py"]);
    write(
        &dir.path().join("renamed.py"),
        "def renamed(x):\n    if x:\n        return x\n    return 0\n",
    );
    git(dir.path(), &["mv", "pure.py", "moved.py"]);
    git(dir.path(), &["rm", "deleted.py"]);
    write(
        &dir.path().join("trimmed.py"),
        "def trimmed(x):\n    return 0\n",
    );
    fs::write(dir.path().join("binary.py"), b"def binary():\0two\n").unwrap();

    let report = json_report(dir.path(), &[]);
    let mut found = symbols(&report);
    found.sort_unstable();
    assert_eq!(found, ["renamed", "trimmed"]);
}

#[test]
fn limits_git_changes_to_the_analysis_path() {
    let dir = tempfile::tempdir().unwrap();
    init(dir.path());
    write(
        &dir.path().join("infra/app.py"),
        "def inside(x):\n    return x\n",
    );
    write(
        &dir.path().join("other/app.py"),
        "def outside(x):\n    return x\n",
    );
    commit_all(dir.path(), "base");
    git(dir.path(), &["checkout", "-b", "topic"]);
    write(
        &dir.path().join("infra/app.py"),
        "def inside(x):\n    if x:\n        return x\n    return 0\n",
    );
    write(
        &dir.path().join("other/app.py"),
        "def outside(x):\n    if x:\n        return x\n    return 0\n",
    );

    let report = json_report(&dir.path().join("infra"), &[]);
    assert_eq!(symbols(&report), ["inside"]);
}

#[test]
fn an_inherited_git_dir_does_not_redirect_the_diff() {
    // Git hooks and `git bisect run` export GIT_DIR. It overrides `-C`, so an
    // inherited value would resolve revisions against the wrong repository.
    let other = tempfile::tempdir().unwrap();
    init(other.path());
    write(
        &other.path().join("other.py"),
        "def other(x):\n    return x\n",
    );
    commit_all(other.path(), "unrelated");

    let dir = tempfile::tempdir().unwrap();
    init(dir.path());
    write(&dir.path().join("app.py"), "def edited(x):\n    return x\n");
    // Unchanged since this repository's own merge base. Diffing against an
    // unrelated tree would surface it as added, so its absence is what proves
    // the correct repository was used.
    write(
        &dir.path().join("untouched.py"),
        "def untouched(x):\n    return x\n",
    );
    commit_all(dir.path(), "base");
    git(dir.path(), &["checkout", "-b", "topic"]);
    write(
        &dir.path().join("app.py"),
        "def edited(x):\n    if x:\n        return x\n    return 0\n",
    );

    let mut command = poly_crap();
    command.env("GIT_DIR", other.path().join(".git")).args([
        "--path",
        dir.path().to_str().unwrap(),
        "--diff-base",
        "main",
        "--format",
        "json",
    ]);
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(symbols(&report), ["edited"]);
}

#[test]
fn rejects_invalid_git_inputs_and_baseline_pairs() {
    let plain = tempfile::tempdir().unwrap();
    write(&plain.path().join("app.py"), "def app(x):\n    return x\n");
    poly_crap()
        .args([
            "--path",
            plain.path().to_str().unwrap(),
            "--diff-base",
            "main",
        ])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("not in a Git repository"));

    let repo = tempfile::tempdir().unwrap();
    init(repo.path());
    write(&repo.path().join("app.py"), "def app(x):\n    return x\n");
    commit_all(repo.path(), "base");
    poly_crap()
        .args([
            "--path",
            repo.path().to_str().unwrap(),
            "--diff-base",
            "missing",
        ])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("invalid Git diff base 'missing'"));

    poly_crap()
        .args([
            "--path",
            repo.path().to_str().unwrap(),
            "--diff-base",
            "main",
            "--baseline",
            "baseline.json",
        ])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "cannot be used with '--baseline <FILE>'",
        ));

    write(
        &repo.path().join(".poly-crap.toml"),
        "fail-regression = true\n",
    );
    poly_crap()
        .args([
            "--path",
            repo.path().to_str().unwrap(),
            "--diff-base",
            "main",
        ])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "--diff-base cannot be combined with fail-regression",
        ));
}
