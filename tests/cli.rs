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
    assert!(languages.contains(&"terraform"));
    let terraform = report["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["language"] == "terraform")
        .unwrap();
    assert!(terraform["crap"].is_null());
    assert_eq!(terraform["metric"], "complexity");

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
