# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with
code in this repository.

## Commands

```sh
cargo fmt --all --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets
cargo package --locked --allow-dirty
```

`uvx prek run -a` runs the same three Rust checks plus the file hygiene hooks,
and is what CI runs.

Run one test by name:

```sh
cargo test --test cli mixed_language_json_matches_schema   # integration
cargo test --lib analysis::tests::rust_impl_is_qualified   # unit
```

Run the tool on itself, which is also the CI gate:

```sh
cargo llvm-cov --locked --all-targets --all-features --lcov --output-path lcov.info
cargo run --locked -- --language rust --coverage lcov.info --threshold 5 --fail-above
```

Toolchain is pinned to Rust 1.88.0 in `rust-toolchain.toml`; the edition is
2024.

## Architecture

One binary over one library. `src/main.rs` parses the CLI and drives a
pipeline; every stage lives in a library module and takes plain data.

```text
config::load        .poly-crap.toml found by walking up from --path
  ↓                 EffectiveConfig merges CLI over file over defaults
analysis            tree-sitter parse → Vec<CodeUnit> (symbol, span, complexity)
  ↓
coverage            parse LCOV / Go profile / JaCoCo → CoverageMap
  ↓
merge               CodeUnit × coverage → Entry (adds coverage, score)
  ↓
report::gate_entries    --allow only; this set decides the exit code
  ↓
report::render_*    human, JSON, or SARIF
```

Two branches hang off that spine:

- `--baseline` swaps the absolute report for `baseline::compare`, which
  produces a `DeltaReport` rendered by `report::render_delta`.
- `--diff-base` runs `git_diff::discover` first, narrowing analysis to changed
  files and then to units whose current span meets a changed line. `git_diff`
  is a binary-private module, not part of the library.

### Rules worth knowing before editing

**Gate set and display set are different.** `report::gate_entries` applies
`--allow` and nothing else, because `--min` and `--top` are display limits —
dropping a row before the gate would let a failing function exit 0. Apply them
with `apply_display_limits` or `limit_delta` after the gate has run. Tests
`display_limits_do_not_hide_*` in `tests/cli.rs` guard this. Baseline runs
gate on both `--fail-regression` and `--fail-above`; `report::DeltaTotals`
counts both before `limit_delta` trims rows.

**`Language` is an array index.** `Language::index()` keys parallel `[_; 6]`
consts: `NAMES` and `EXTENSIONS` in `model.rs`, `GRAMMAR_LOADERS`,
`DECLARATION_KINDS`, `SYMBOL_SEPARATORS` in `analysis.rs`. Adding a language
means touching every one of them plus `ALL` and `ALIASES`.

**Complexity stops at nested callables.** `count_decisions` refuses to descend
into another callable, so an inner function never inflates its parent. Named
and assigned callables become their own units through `named_unit`; anonymous
callbacks are scored nowhere. Each language's declaration kinds live in
`DECLARATION_KINDS`, and each assignment shape in `ASSIGNMENT_SPECS`.

**Test code is excluded two ways.** Other languages rely on the path globs in
`config::DEFAULT_EXCLUDES`; Rust keeps tests in the same file, so
`TestAttributes` in `analysis.rs` drops `#[cfg(test)]` and `#[test]` items
while walking siblings.

**Path matching is fuzzy on purpose.** `merge::lookup_coverage` tries a
canonical absolute match, then the longest *unique* path suffix, because
coverage tools report paths relative to their own roots. Ambiguous suffixes
stay unmatched, and a one-component suffix is trusted only when the reported
path is itself a bare file name (`TRUSTED_SUFFIX` in `merge.rs`), so
`pkg_a/util.py` never borrows `pkg_b/util.py`. `report::matches_path` tries
both the reported path and its
root-relative form for the same reason.

**JSON output is a published contract.** `schemas/report-v1.json` and
`schemas/delta-v1.json` are validated against real output in `tests/cli.rs`
with `jsonschema`. Changing a serialized field means updating the schema in the
same change.

**Exit codes carry meaning.** 0 clean, 1 a requested gate failed, 2 anything
wrong with usage, input, or output. `main.rs` maps these; don't `bail!` for a
gate failure or return 1 for a bad argument.

## Conventions

- CI fails the build when any function scores above CRAP 5. Keep functions
  short and test them, or the repo rejects its own change.
- No test may need network access, credentials, or machine state. Git fixtures
  must set `commit.gpgsign false`, or signing configs break them.
- Conventional Commit subjects. The project squash-merges, so the PR title
  becomes the commit on `main` and drives release-please.
- Never add `Co-Authored-By` or AI attribution to commits.
