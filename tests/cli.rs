use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::*;
use serde_json::Value;
use std::fs;
use std::path::Path;

fn write(path: &Path, value: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, value).unwrap();
}

#[test]
fn mixed_language_json_matches_schema() {
    let dir = tempfile::tempdir().unwrap();
    let rust = dir.path().join("src/lib.rs");
    write(
        &rust,
        "pub fn run(x: bool) -> u8 { if x { 1 } else { 0 } }\n",
    );
    write(
        &dir.path().join("web/app.ts"),
        "export const choose = (x: boolean) => x ? 1 : 0;\n",
    );
    write(
        &dir.path().join("main.tf"),
        "resource \"test_item\" \"main\" { count = var.on ? 1 : 0 }\n",
    );
    write(&dir.path().join("infra.hcl"), "block \"x\" {}\n");
    let coverage = dir.path().join("coverage.lcov");
    write(
        &coverage,
        &format!(
            "SF:{}\nDA:1,1\nend_of_record\nSF:{}\nDA:1,0\nend_of_record\n",
            rust.display(),
            dir.path().join("web/app.ts").display()
        ),
    );

    let output = cargo_bin_cmd!("poly-crap")
        .args([
            "--path",
            dir.path().to_str().unwrap(),
            "--coverage",
            coverage.to_str().unwrap(),
            "--jobs",
            "1",
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let languages: Vec<_> = report["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["language"].as_str().unwrap())
        .collect();
    assert!(languages.contains(&"rust"));
    assert!(languages.contains(&"typescript"), "{report}");
    assert!(
        !languages.contains(&"terraform"),
        "Terraform is no longer scanned: {report}"
    );
    assert_eq!(report["entries"].as_array().unwrap().len(), 2);

    let schema: Value = serde_json::from_str(include_str!("../schemas/report-v1.json")).unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    assert!(validator.is_valid(&report));
}

#[test]
fn threshold_gate_and_config_precedence_work() {
    let dir = tempfile::tempdir().unwrap();
    write(
        &dir.path().join("app.py"),
        "def risky(a, b, c):\n    if a:\n        if b:\n            if c:\n                return 1\n    return 0\n",
    );
    write(
        &dir.path().join(".poly-crap.toml"),
        "threshold = 1000.0\nfail-above = true\n",
    );
    cargo_bin_cmd!("poly-crap")
        .args(["--path", dir.path().to_str().unwrap()])
        .assert()
        .success();
    cargo_bin_cmd!("poly-crap")
        .args(["--path", dir.path().to_str().unwrap(), "--threshold", "10"])
        .assert()
        .code(1);
}

#[test]
fn syntax_error_warns_when_another_file_parses() {
    let dir = tempfile::tempdir().unwrap();
    write(
        &dir.path().join("good.js"),
        "function good() { return 1; }\n",
    );
    write(&dir.path().join("bad.js"), "function {\n");
    cargo_bin_cmd!("poly-crap")
        .args(["--path", dir.path().to_str().unwrap()])
        .assert()
        .success()
        .stderr(predicate::str::contains("warning:"));
}

#[test]
fn baseline_delta_matches_schema_and_can_fail() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("app.py");
    let baseline = dir.path().join("baseline.json");
    write(&source, "def run(x):\n    return x\n");
    cargo_bin_cmd!("poly-crap")
        .args([
            "--path",
            dir.path().to_str().unwrap(),
            "--format",
            "json",
            "--output",
            baseline.to_str().unwrap(),
        ])
        .assert()
        .success();
    write(
        &source,
        "def run(x):\n    if x:\n        if x > 1:\n            return x\n    return 0\n",
    );
    let output = cargo_bin_cmd!("poly-crap")
        .args([
            "--path",
            dir.path().to_str().unwrap(),
            "--baseline",
            baseline.to_str().unwrap(),
            "--fail-regression",
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["entries"][0]["status"], "regressed");
    let schema: Value = serde_json::from_str(include_str!("../schemas/delta-v1.json")).unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    assert!(validator.is_valid(&report));
}

#[test]
fn config_is_found_from_a_subdirectory_with_a_relative_path() {
    // `--path` defaults to `.`, and `Path::ancestors` on a relative path stops
    // at that path, so the search has to start from an absolute directory for
    // a config in a parent directory to be reachable at all.
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join(".poly-crap.toml"), "threshold = 1.0\n");
    write(
        &dir.path().join("sub/app.py"),
        "def risky(x):\n    if x:\n        return 1\n    return 0\n",
    );
    cargo_bin_cmd!("poly-crap")
        .current_dir(dir.path().join("sub"))
        .args(["--path", ".", "--fail-above"])
        .assert()
        .code(1);
}

#[test]
fn display_limits_do_not_hide_threshold_failures_from_the_gate() {
    let dir = tempfile::tempdir().unwrap();
    write(
        &dir.path().join("app.py"),
        "def simple():\n    return 0\n\ndef run(x):\n    if x:\n        if x > 1:\n            if x > 2:\n                return 1\n    return 0\n",
    );
    // `--min` and `--top` only choose printed rows. `run` scores above the
    // threshold, so it has to fail the build even when no row survives, and
    // the summary has to count both functions rather than what it printed.
    for limit in [["--min", "1000"], ["--top", "1"]] {
        cargo_bin_cmd!("poly-crap")
            .args([
                "--path",
                dir.path().to_str().unwrap(),
                "--missing",
                "pessimistic",
                "--threshold",
                "5",
                "--fail-above",
            ])
            .args(limit)
            .assert()
            .code(1)
            .stdout(predicate::str::contains("1 of 2 function(s) exceed"));
    }
}

#[test]
fn display_limits_do_not_hide_regressions_from_the_gate() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("app.py");
    let baseline = dir.path().join("baseline.json");
    write(
        &source,
        "def run(x):\n    if x:\n        return 1\n    return 0\n",
    );
    cargo_bin_cmd!("poly-crap")
        .args([
            "--path",
            dir.path().to_str().unwrap(),
            "--format",
            "json",
            "--output",
            baseline.to_str().unwrap(),
        ])
        .assert()
        .success();
    write(
        &source,
        "def run(x):\n    if x:\n        if x > 1:\n            if x > 2:\n                if x > 3:\n                    return 1\n    return 0\n",
    );
    // `--min` drops the low-scoring baseline row. If that filter reaches the
    // baseline, the regressed function matches nothing and is reported as new,
    // which `--fail-regression` does not gate on.
    cargo_bin_cmd!("poly-crap")
        .args([
            "--path",
            dir.path().to_str().unwrap(),
            "--baseline",
            baseline.to_str().unwrap(),
            "--fail-regression",
            "--min",
            "10",
        ])
        .assert()
        .code(1)
        .stdout(predicate::str::contains("regressed"));
}

#[test]
fn allow_path_globs_suppress_reported_entries() {
    let dir = tempfile::tempdir().unwrap();
    write(
        &dir.path().join("vendor/dep.py"),
        "def vendored(x):\n    if x:\n        return 1\n    return 0\n",
    );
    write(
        &dir.path().join("app.py"),
        "def kept(x):\n    if x:\n        return 1\n    return 0\n",
    );
    let output = cargo_bin_cmd!("poly-crap")
        .args([
            "--path",
            dir.path().to_str().unwrap(),
            "--no-default-excludes",
            "--allow",
            "vendor/**",
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let symbols: Vec<_> = report["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["symbol"].as_str().unwrap())
        .collect();
    assert_eq!(symbols, ["kept"], "{report}");
}

#[test]
fn malformed_coverage_exits_two() {
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("app.js"), "function run() {}\n");
    let coverage = dir.path().join("bad.txt");
    write(&coverage, "not a coverage report\n");
    cargo_bin_cmd!("poly-crap")
        .args([
            "--path",
            dir.path().to_str().unwrap(),
            "--coverage",
            coverage.to_str().unwrap(),
        ])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("unknown coverage format"));
}
