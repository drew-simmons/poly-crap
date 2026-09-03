# poly-crap

[![CI](https://img.shields.io/github/actions/workflow/status/drew-simmons/poly-crap/ci.yml?branch=main&label=CI)](https://github.com/drew-simmons/poly-crap/actions/workflows/ci.yml)
[![Latest release](https://img.shields.io/github/v/release/drew-simmons/poly-crap?label=release)](https://github.com/drew-simmons/poly-crap/releases/latest)
[![License](https://img.shields.io/github/license/drew-simmons/poly-crap)](LICENSE)

Poly-crap finds the functions in a codebase that are both complex and poorly
tested. It parses your source with tree-sitter, reads the coverage report your
test runner already writes, and scores every function with the CRAP metric.
It supports TypeScript, JavaScript, Python, Go, Rust, and Java, and it ships
as one binary with no runtime dependencies.

Use it three ways:

- In a pull request, to fail the build when a changed function is too complex
  for its tests. See [Changed functions](#changed-functions).
- Against a baseline, to fail the build when any score gets worse. See
  [Baselines](#baselines).
- On a whole repository, to find the functions most worth testing or splitting
  next.

## Install

### Installer (macOS and Linux)

```sh
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/drew-simmons/poly-crap/releases/latest/download/poly-crap-installer.sh \
  | sh
```

> [!NOTE]
> The installer downloads a prebuilt release, checks its SHA-256 checksum, and
> puts `poly-crap` in `~/.cargo/bin`. Archives and checksums are also on
> [GitHub Releases](https://github.com/drew-simmons/poly-crap/releases). To pin
> a version, replace `latest/download` with `download/v0.6.0`.

### Build from source

Building from source requires Rust 1.88 or newer:

```sh
git clone https://github.com/drew-simmons/poly-crap.git
cd poly-crap
cargo install --path . --locked
```

### Release targets

| Platform | Architectures |
| --- | --- |
| macOS | Apple Silicon (`aarch64`), Intel (`x86_64`) |
| Linux with glibc | `aarch64`, `x86_64` |
| Linux with musl/Alpine | `x86_64` |

There is no Windows build, and Windows is not tested.

## Quick start

1. Run your tests with coverage on. Poly-crap reads the report; it never runs
   tests itself. [Coverage reports](#coverage-reports) lists the command for
   each stack.

2. Run poly-crap from the repository root. It finds a report at any of the
   default locations on its own:

   ```sh
   poly-crap
   ```

3. Read the table. Each row is a function over the threshold, and the last
   column names the lines a test never reached:

   ```text
   CRAP results
     CRAP     CC  Coverage  Language    Symbol  Location  Uncovered
         6.0    4.0     50.0%  python  run  ./app.py:4  6-8
     1 more function(s) not shown; adjust --min or --top to list them.
   1 of 2 function(s) exceed CRAP threshold 5.0.
   Coverage scope: 1 analyzed file(s), 1 coverage source file(s), 1 matched, 0 source-only, 0 coverage-only.
   ```

4. Gate a branch. This scores only the functions the branch changed and exits
   `1` when one is over the threshold:

   ```sh
   poly-crap --diff-base main --fail-above
   ```

## How the score works

Poly-crap uses the Change Risk Anti-Patterns metric:

```text
CRAP(m) = complexity(m)² × (1 - coverage(m) / 100)³ + complexity(m)
```

`complexity` is the function's cyclomatic complexity: one, plus one for each
decision point. [Complexity rules](#complexity-rules) lists what counts.
`coverage` is the share of the function's executable lines that a test ran.

Two things follow from the formula:

- At 100% coverage the score equals the complexity. Coverage can only remove
  the first term, never the second, so a threshold also caps complexity.
- The coverage term is cubed. Half coverage leaves an eighth of the penalty;
  80% coverage leaves under 1%. The first tests on a function pay off the most.

| Complexity | 0% | 50% | 80% | 100% |
| --- | --- | --- | --- | --- |
| 1 | 2.0 | 1.1 | 1.0 | 1.0 |
| 2 | 6.0 | 2.5 | 2.0 | 2.0 |
| 4 | 20.0 | 6.0 | 4.1 | 4.0 |
| 6 | 42.0 | 10.5 | 6.3 | 6.0 |
| 10 | 110.0 | 22.5 | 10.8 | 10.0 |

### Choosing a threshold

The default threshold is `5`. Under it, a function may have at most five
decision points however well it is tested, a function with four needs more
than 60% coverage, and a function with two needs about 10%. This repository
holds itself to that number. It suits a small, well-tested codebase that wants
to stay that way.

The threshold the metric's authors proposed is `30`. It tolerates a
complexity of 30 when fully covered, or a complexity of 5 with no coverage at
all. It suits an existing codebase that wants to catch the worst offenders
first.

To pick one for your code:

1. Run once with `--min 0` to see every score, or `--top 20` for the worst.
2. Put a `threshold` you can meet today in `.poly-crap.toml`, and lower it as
   the code improves.
3. Gate pull requests with `--diff-base` in the meantime, so new code meets the
   bar even where old code does not. A [baseline](#baselines) does the same for
   the whole repository.

## Coverage reports

Poly-crap reads three formats and detects each from its contents:

- LCOV, from `cargo llvm-cov`, c8, Istanbul, coverage.py, and most others.
- Go cover profiles, from `go test -coverprofile`.
- JaCoCo XML, from the Maven and Gradle plug-ins.

LCOV and JaCoCo give line coverage. Go profiles give statement coverage,
weighted by statement count. JSON output records which basis each function
used.

### Producing a report

Rust:

```sh
cargo llvm-cov --lcov --output-path coverage.lcov
```

TypeScript or JavaScript with c8:

```sh
npx c8 --reporter=lcov npm test
```

Python:

```sh
coverage run -m pytest
coverage lcov -o coverage.lcov
```

Go:

```sh
go test -coverprofile=coverage.out ./...
```

Java with Maven:

```sh
mvn verify
```

Java with Gradle:

```sh
gradle test jacocoTestReport
```

Every one of these writes to a location poly-crap searches on its own.

### Auto-discovery

When neither the command line nor `.poly-crap.toml` names a report, poly-crap
looks under `--path` for the files coverage tools write by default, in this
order:

```text
coverage.lcov
lcov.info
coverage/lcov.info
coverage/coverage.lcov
coverage.out
target/site/jacoco/jacoco.xml
build/reports/jacoco/test/jacocoTestReport.xml
```

It uses every report it finds and names them on stderr. When it finds none, it
warns that every function will follow the `--missing` policy, which by default
scores it as 0% covered. Pass `--no-auto-coverage` to skip the search.

> [!WARNING]
> Poly-crap does not check that a report is newer than the source. A report
> left over from last week scores today's code. Regenerate it before you scan.

### Merging reports and matching paths

Pass `--coverage` more than once to merge reports, for example one per
language. Where two reports cover the same line, a hit in either counts.

Coverage tools write paths relative to their own roots, so poly-crap matches a
source file to a report entry in three steps:

1. The same path, as written.
2. The same file after resolving symlinks, when both paths are absolute.
3. The longest path suffix shared with exactly one report entry. A suffix must
   cover at least the file name and its directory, unless the report names the
   file with no directory at all, as `SF:app.py` does. Without that rule a
   file the tests never ran, such as `pkg_a/util.py`, could borrow the coverage
   of `pkg_b/util.py`.

A suffix shared by two report entries matches neither. Poly-crap warns on
stderr when source files and report entries do not line up, and the
`Coverage scope` line at the end of the report counts both sides.

Poly-crap does not read JavaScript source maps. For TypeScript, the coverage
tool must report the original `.ts` paths, which c8 and Vitest do by default.

### Functions without coverage

A function has no coverage when its file is not in any report, or when the
report has no executable lines inside the function. `--missing` decides what
happens:

- `pessimistic`, the default, scores it as 0% covered.
- `optimistic` scores it as 100% covered.
- `skip` leaves it out of the report and the gates.

In every case the function's `coverage` is `null` in JSON and `N/A` in the
table, so you can tell a measured 0% from a missing one.

## Reading the output

### Human output

```text
CRAP results
  CRAP     CC  Coverage  Language    Symbol  Location  Uncovered
      6.0    4.0     50.0%  python  run  ./app.py:4  6-8
  1 more function(s) not shown; adjust --min or --top to list them.
1 of 2 function(s) exceed CRAP threshold 5.0.
Coverage scope: 1 analyzed file(s), 1 coverage source file(s), 1 matched, 0 source-only, 0 coverage-only.
```

- **CRAP** is the score.
- **CC** is the cyclomatic complexity.
- **Coverage** is the share of the function's executable lines that a test
  ran, or `N/A` when no report covers the function.
- **Symbol** is the qualified name: `Class.method`, `Type::method` in Rust,
  `Type.Method` for a Go method, `Class.method(int,String)` in Java, and
  `outer.inner` for a nested function.
- **Location** is the file and the line the function starts on.
- **Uncovered** lists the line ranges inside the function that no test ran, up
  to six, then a count of the rest.

The table holds the functions over the threshold, sorted by score. The line
under it says how many rows it left out. `--min <SCORE>` sets a different
floor, so `--min 0` lists every function; `--top <N>` caps the rows; `--sort
file` groups rows by file. None of them change the summary line or the exit
code. `--summary` prints the summary lines with no table.

With `--diff-base`, a first line names the merge base and counts the changed
files and the functions selected from them.

### JSON

`--format json` writes every function, not just the failures, so a report can
serve as a [baseline](#baselines). Each entry carries the fields the table
shows plus `end_line`, `coverage_basis` (`line`, `statement`, or `null`), and
`uncovered` as a list of `{start, end}` ranges. A `diagnostics` object counts
files and lists up to twenty warnings. The document names its schema,
[report-v1.json](schemas/report-v1.json), and a baseline comparison uses
[delta-v1.json](schemas/delta-v1.json).

Warnings go to stderr, so `poly-crap --format json | jq` stays clean. Merging
the streams with `2>&1` breaks the JSON.

### SARIF

`--format sarif` writes SARIF 2.1.0 with one result per function over the
threshold, under the rule `poly-crap/crap-threshold`, for GitHub code scanning
and editors that read SARIF. It cannot be combined with `--baseline`.

### Exit codes

| Code | Meaning |
| --- | --- |
| `0` | The scan completed and no requested gate failed. |
| `1` | `--fail-above` or `--fail-regression` failed. |
| `2` | Usage, input, parsing, coverage, or output failed. |

Without a gate flag, poly-crap always exits `0`, however bad the scores. A
source file with a syntax error produces a warning and is skipped; the run
fails only when candidate files exist and none of them parse.

## Gating changes

### Changed functions

Name the base revision of a branch, and poly-crap scores only the functions
the branch touched:

```sh
poly-crap --diff-base main --fail-above
```

It finds the merge base of the revision and `HEAD`, then takes committed
changes, staged and unstaged edits, and untracked files that `.gitignore` does
not cover. A function is selected when its current line range meets an added
or changed line. Its score still comes from the whole function and its current
coverage, so an edit to one line of an untested twenty-branch function fails
the gate. That is the point: the function is now yours.

Renamed files are followed. Deleted functions and binary files are ignored.
Git must be installed, `--path` must be inside a Git repository, and the
revision must resolve to a commit. In a pull request checkout the branch you
want is usually `origin/main`; see [GitHub Actions](#github-actions).

`--diff-base` cannot be combined with `--baseline`.

### Baselines

For an existing codebase, save a JSON report on the main branch:

```sh
poly-crap --format json --output poly-crap.json
```

Then compare a change against it:

```sh
poly-crap --baseline poly-crap.json --fail-regression --fail-above
```

Each current function is matched to the baseline by file, name, and position,
with allowances for a baseline written from a different root. Every function
gets one status:

- `regressed`: the score rose by more than `--epsilon`, default `0.01`.
- `improved`: the score fell by more than `--epsilon`.
- `unchanged`: neither. The table hides these rows; JSON keeps them.
- `new`: nothing in the baseline matched.
- `moved`: the same function, found in another file.
- `removed`: a baseline function nothing matched.

`--fail-regression` fails the run on any `regressed` function. It says nothing
about a `new` one, because there is no earlier score to rise from, so pair it
with `--fail-above` to hold new functions to the threshold. A baseline written
by an older poly-crap may not parse; regenerate it.

`--allow` applies to the baseline too, so an allowed function never appears as
`removed`.

## What gets scored

### Languages

| Language | Extensions | `--language` |
| --- | --- | --- |
| JavaScript | `.js` `.jsx` `.mjs` `.cjs` | `javascript`, `js` |
| TypeScript | `.ts` `.tsx` `.mts` `.cts` | `typescript`, `ts` |
| Python | `.py` | `python`, `py` |
| Go | `.go` | `go` |
| Rust | `.rs` | `rust` |
| Java | `.java` | `java` |

All six are scanned by default. Repeat `--language` to narrow a run. The
configuration file takes the long names only.

### Functions

Poly-crap scores every named function, method, and constructor, and every
function value assigned to a name: a JavaScript arrow function in a `const`,
a Python `lambda`, a Go `func` literal, a Rust closure in a `let`, or a Java
lambda in a local variable. Each is scored on its own; an inner function never
adds to its parent's complexity.

Anonymous callbacks, such as the argument to `.map()` or an `export default
function () {}`, get no score. In callback-heavy code, such as React
components or Express handlers, that leaves real complexity unscored; name the
callback to score it.

Test code is left out. Rust items under `#[cfg(test)]` or `#[test]` are
skipped, and the other languages keep tests in files that the
[default excludes](#excluded-paths) drop. Java methods with no body, such as
interface signatures, are skipped as well.

### Complexity rules

Every function starts at `1`. Each of these adds one:

- `if`, `elif`, and `else if`.
- A ternary or conditional expression.
- Each loop: `for`, `while`, `do`, and Rust's `loop`.
- Each `case` of a `switch`, arm of a `match`, or case of a Go `select`.
  Default arms add nothing.
- Each `catch` or `except` clause.
- Each `&&`, `||`, `and`, and `or`.
- Each `for` and `if` clause in a Python comprehension.

These add nothing: `??` and `?.`, Rust's `?` and `let … else`, `finally`,
early returns, and a nested function, which is scored on its own.

### Excluded paths

The scanner respects `.gitignore`, skips hidden directories, and by default
skips the directories `node_modules`, `target`, `vendor`, `.terraform`,
`dist`, `build`, `tests`, `test`, `__tests__`, `src/test`, `benches`, and
`examples` at any depth, plus the test files `*_test.go`, `*.test.*`,
`*.spec.*`, `test_*.py`, and `*_test.py`.

`--exclude <GLOB>` adds a pattern; the configuration key `default-excludes`
replaces the built-in list; `--no-default-excludes` drops it. Globs match
paths relative to `--path`, and `*` does not cross a directory separator, so
write `src/generated/**` rather than `src/generated/*`. An excluded file is
never parsed and never counted.

### Allow lists

`--allow <GLOB>` suppresses functions after scoring, so they leave the table,
the gates, and the summary counts. A pattern with a `/` or `**` matches file
paths, relative to `--path` or as reported. Any other pattern matches the
qualified symbol, and `*` crosses `.` and `::`:

```sh
poly-crap --allow 'legacy::*' --allow 'vendor/**'
```

Java symbols carry their parameter types, so match them with a glob:
`'Parser.parse(*'`. Matching is case sensitive.

## Options

Every flag has a key in `.poly-crap.toml` unless noted. The command line wins
over the file, and the file over the defaults, except that `exclude` and
`allow` lists are joined and a boolean is on when either side sets it.

### Scope

- `--path <DIR>`, default `.`. The directory to scan and the root that
  relative paths, globs, and auto-discovery use. Command line only.
- `--language <NAME>`, key `languages`. Repeatable; the file takes a list.
  Scans all six languages by default.
- `--exclude <GLOB>`, key `exclude`. Repeatable; see
  [Excluded paths](#excluded-paths).
- `--no-default-excludes`. Drops the built-in exclude list. Command line only;
  the file uses `default-excludes = [...]` to replace the list instead.
- `--allow <GLOB>`, key `allow`. Repeatable; see [Allow lists](#allow-lists).
- `--diff-base <REV>`. Scores only functions changed since the merge base with
  `REV`. Command line only.

### Coverage

- `--coverage <FILE>`, key `coverage`. Repeatable; reports are merged. Paths
  in the file are relative to the working directory.
- `--no-auto-coverage`. Skips the default-location search when no report is
  named. Command line only.
- `--missing <POLICY>`, key `missing`, default `pessimistic`. Also
  `optimistic` or `skip`.

### Gates

- `--threshold <SCORE>`, key `threshold`, default `5`. The score a function
  must stay at or under. Also decides which rows the table and SARIF show.
- `--fail-above`, key `fail-above`. Exit `1` when any function is over the
  threshold.
- `--baseline <FILE>`. A JSON report from an earlier run to compare against.
  Command line only.
- `--fail-regression`, key `fail-regression`. Exit `1` when any score rose
  from the baseline. Requires `--baseline`.
- `--epsilon <DELTA>`, key `epsilon`, default `0.01`. A score change within
  this much counts as unchanged.

### Output

- `--format <FORMAT>`, key `format`, default `human`. Also `json` or `sarif`.
- `--output <FILE>`. Write the report to a file instead of stdout. Command
  line only.
- `--min <SCORE>`, key `min`. Show rows scoring at least this much. Human
  output otherwise shows rows over the threshold; JSON shows every row.
- `--top <N>`, key `top`. Show at most `N` rows, highest scores first.
- `--sort <ORDER>`, key `sort`, default `crap`. Also `file`.
- `--summary`, key `summary`. Human output without the table.

`--min`, `--top`, `--sort`, and `--summary` change what is printed, never
the gates or the summary counts.

### Performance

- `--jobs <N>`, key `jobs`. Threads for parsing. Defaults to the number of
  cores.

### Configuration file

Poly-crap looks for `.poly-crap.toml` in `--path`, then in each parent
directory, and uses the first one it finds. Unknown keys are an error, so a
typo cannot silently do nothing.

```toml
languages = ["rust", "typescript", "python"]
threshold = 30.0
missing = "pessimistic"
fail-above = true
exclude = ["src/generated/**"]
allow = ["legacy::*", "vendor/**"]
sort = "file"
jobs = 4
```

## CI recipes

### GitHub Actions

Score the functions a pull request changed and fail the check when one is
over the threshold. The checkout needs the full history so the merge base
with the target branch exists; a default shallow checkout does not have it.

```yaml
name: poly-crap

on:
  pull_request:

jobs:
  crap:
    runs-on: ubuntu-latest
    permissions:
      contents: read
    steps:
      - uses: actions/checkout@v7
        with:
          fetch-depth: 0
      # Run your tests with coverage first, for example:
      - run: go test -coverprofile=coverage.out ./...
      - name: Install poly-crap
        run: |
          curl --proto '=https' --tlsv1.2 -LsSf \
            https://github.com/drew-simmons/poly-crap/releases/latest/download/poly-crap-installer.sh \
            | sh
          echo "$HOME/.cargo/bin" >> "$GITHUB_PATH"
      - name: Gate changed functions
        run: poly-crap --diff-base "origin/${{ github.base_ref }}" --fail-above
```

To show failures as code-scanning alerts on the pull request, write SARIF and
upload it. This needs `security-events: write` in the job's permissions. Run
from the repository root so the paths in the file are repository-relative.

```yaml
      - name: Write SARIF
        run: poly-crap --format sarif --output poly-crap.sarif
      - uses: github/codeql-action/upload-sarif@v3
        with:
          sarif_file: poly-crap.sarif
```

Pin the installer to a release, as in [Install](#install), when you want the
check to change only when you say so.

### Pre-commit hook

With [prek](https://github.com/j178/prek) or pre-commit, a local hook gates
what you are about to commit. Poly-crap does not run tests, so it reads
whatever report is on disk; run the tests first, or accept that the report
may be a little behind.

```yaml
repos:
  - repo: local
    hooks:
      - id: poly-crap
        name: poly-crap
        entry: poly-crap --diff-base main --fail-above
        language: system
        pass_filenames: false
        always_run: true
```

## Troubleshooting

**Every function shows `N/A` coverage and a high score.** No report matched.
Check stderr: `warning: no coverage report found` means none was found, so
pass `--coverage`; `coverage scope mismatch` means one was found but its paths
do not line up with the source. Compare a path in the report with the path
poly-crap prints, and see
[Merging reports and matching paths](#merging-reports-and-matching-paths). For
TypeScript, make sure the report names `.ts` files, not compiled `.js`.

**The scores look wrong after a refactor.** The report is stale. Poly-crap
scores today's source against whatever report it finds; regenerate the report.

**`none of the N candidate source files parsed successfully`.** Every file
of a language failed to parse, which usually means the files are not what
their extension says, or use syntax the grammar does not know. A single
unparseable file only warns.

**`invalid Git diff base 'main'`.** The revision does not exist locally. In
CI, check out with full history and name the remote branch, as in
[GitHub Actions](#github-actions). Locally, `git fetch origin main` and use
`origin/main`.

**`--diff-base` reports functions I did not touch.** Their line range meets a
changed line, which includes a comment or a blank line inside the function.
The score is for the whole function, so the report is telling you what you now
own.

**`jq` fails on the JSON.** Warnings went to the same stream. Do not merge
stderr into stdout, or write the report with `--output`.

**A function I allowed still fails the gate.** Java symbols carry parameter
types and Rust symbols use `::`, so `--allow 'D.risky'` matches nothing where
`--allow 'D.risky(*'` does. Symbol patterns are case sensitive.

**The table is empty but the summary says functions exceed the threshold.**
`--top` or `--min` hid the rows; the summary counts every function. Drop the
limit, or pass `--min 0`.

## Development

The project uses Rust 1.88.0. Before submitting a change, run:

```sh
cargo fmt --all --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets
cargo package --locked --allow-dirty
```

CI also runs poly-crap on itself with a threshold of `5` and fails the build
when a function is over it. [CLAUDE.md](CLAUDE.md) describes the architecture
and the rules the code follows. The repository also ships a
[Claude Code skill](.claude/skills/poly-crap/SKILL.md) that wraps the binary
and smoke-tests a checkout.

See [CONTRIBUTING.md](CONTRIBUTING.md) for contribution guidance,
[SECURITY.md](SECURITY.md) for vulnerability reporting, and
[RELEASING.md](RELEASING.md) for maintainer release steps. See
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) for cargo-crap attribution.

## License

MIT
