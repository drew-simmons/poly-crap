# poly-crap

[![CI](https://img.shields.io/github/actions/workflow/status/drew-simmons/poly-crap/ci.yml?branch=main&label=CI)](https://github.com/drew-simmons/poly-crap/actions/workflows/ci.yml)
[![Latest release](https://img.shields.io/github/v/release/drew-simmons/poly-crap?label=release)](https://github.com/drew-simmons/poly-crap/releases/latest)
[![License](https://img.shields.io/github/license/drew-simmons/poly-crap)](LICENSE)

Find functions that combine high complexity with low test coverage across a
polyglot codebase. Poly-crap supports TypeScript, JavaScript, Python, Go, Rust,
and Java. It also reports a separate complexity score for Terraform blocks.

Poly-crap uses the Change Risk Anti-Patterns metric:

```text
CRAP(m) = complexity(m)² × (1 - coverage(m) / 100)³ + complexity(m)
```

The default threshold is `30`. A high score points to code that needs simpler
control flow, more useful tests, or both.

## Install

### Installer (macOS and Linux)

```sh
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/drew-simmons/poly-crap/releases/latest/download/poly-crap-installer.sh \
  | sh
```

> [!NOTE]
> The installer downloads a prebuilt release and checks its SHA-256 checksum.
> You can also download archives and checksums from
> [GitHub Releases](https://github.com/drew-simmons/poly-crap/releases).

### Build from source

Building from source requires Rust 1.88 or newer:

```sh
git clone https://github.com/drew-simmons/poly-crap.git
cd poly-crap
cargo install --path . --locked
```

### Supported release targets

| Platform | Architectures |
| --- | --- |
| macOS | Apple Silicon (`aarch64`), Intel (`x86_64`) |
| Linux with glibc | `aarch64`, `x86_64` |
| Linux with musl/Alpine | `x86_64` |

## Usage

```sh
poly-crap --path . --coverage coverage.lcov
poly-crap --path . --coverage coverage.out --fail-above
poly-crap --path . --coverage jacoco.xml --format sarif --output poly-crap.sarif
```

Poly-crap detects LCOV, Go cover profiles, and JaCoCo XML from their contents.
Pass `--coverage` more than once to merge reports. The tool reads reports but
does not run tests, builds, Terraform, or report converters.

### Create coverage reports

Rust:

```sh
cargo llvm-cov --lcov --output-path coverage.lcov
poly-crap --coverage coverage.lcov
```

TypeScript or JavaScript with c8:

```sh
npx c8 --reporter=lcov npm test
poly-crap --coverage coverage/lcov.info
```

Python:

```sh
coverage run -m pytest
coverage lcov -o coverage.lcov
poly-crap --coverage coverage.lcov
```

Go:

```sh
go test -coverprofile=coverage.out ./...
poly-crap --coverage coverage.out
```

Java with the JaCoCo Maven plug-in:

```sh
mvn verify
poly-crap --coverage target/site/jacoco/jacoco.xml
```

LCOV and JaCoCo use executable-line coverage. Go profiles keep Go's weighted
statement coverage. JSON output records the coverage basis for each function.

### Languages and filters

Poly-crap scans all supported file types by default. Narrow a run with a
repeatable language flag:

```sh
poly-crap --language ts --language py --coverage coverage.lcov
```

Accepted short names include `js`, `ts`, `py`, and `tf`. The scanner respects
`.gitignore` and skips common dependency, build, generated, and test paths.
Use `--exclude`, `--allow`, or `--no-default-excludes` to change the scope.

### Git diff checks

Limit a run to functions and Terraform blocks changed on a local branch by
naming its base revision:

```sh
poly-crap --diff-base main --coverage coverage.lcov --fail-above
```

Poly-crap finds the merge base of the named revision and `HEAD`, then includes
committed branch changes, staged and unstaged changes, and untracked files. A
unit enters the report when its current line range meets an added or changed
line. Its score still uses the whole current function or block and its current
coverage.

Git diff mode ignores deleted units and works with the normal language, path,
exclude, allow, output, and threshold options. It cannot be combined with
`--baseline` or `--fail-regression`. Git must be installed, the analysis path
must be inside a Git repository, and the base must name a commit.

### Missing coverage

Functions with no matching coverage use the pessimistic policy and count as
0% covered. Other choices are:

```sh
poly-crap --missing optimistic
poly-crap --missing skip
```

When source and coverage paths differ, poly-crap matches canonical absolute
paths first and then the longest unique path suffix. It warns when source and
coverage scopes do not match.

### Baseline checks

For an existing codebase, save a JSON baseline on the main branch:

```sh
poly-crap --coverage coverage.lcov --format json --output poly-crap.json
```

Compare a change against it:

```sh
poly-crap \
  --coverage coverage.lcov \
  --baseline poly-crap.json \
  --fail-regression
```

The baseline check covers CRAP scores and Terraform complexity. New, improved,
regressed, unchanged, moved, and removed units appear in delta JSON. Absolute
thresholds only apply to CRAP scores.

### Terraform

Terraform has no common line-coverage report, so `.tf` files get a separate
complexity report and no CRAP score. Poly-crap counts conditional and `for`
expressions, Boolean branches, dynamic blocks, `count`, `for_each`, and
validation or condition blocks.

Version 1 does not scan `.tf.json`, `.tfvars`, general `.hcl`, or
`.tftest.hcl`. SARIF omits Terraform because version 1 has no absolute
Terraform complexity gate.

### Configuration

Put `.poly-crap.toml` in the project root or a parent directory. Command-line
values take precedence, and unknown keys cause an error.

```toml
languages = ["rust", "typescript", "python", "terraform"]
threshold = 30.0
missing = "pessimistic"
fail-above = true
exclude = ["src/generated/**"]
allow = ["legacy::*", "vendor/**"]
sort = "file"
jobs = 4
```

### Output and exit codes

`--format` accepts `human`, `json`, or `sarif`. The JSON formats use the
published [absolute](schemas/report-v1.json) and
[delta](schemas/delta-v1.json) schemas.

| Code | Meaning |
| --- | --- |
| `0` | The scan completed and no requested gate failed. |
| `1` | `--fail-above` or `--fail-regression` failed. |
| `2` | Usage, input, parsing, coverage, or output failed. |

Source files with syntax errors produce warnings and remain in JSON
diagnostics. A run fails only when candidate files exist and none parse.

### Complexity rules

Every named function, method, constructor, or assigned function value starts
at `1`. Conditional branches, ternaries, loops, non-default match or switch
arms, catch or except clauses, short-circuit Boolean operators, and
comprehension generators or filters add one. Anonymous callbacks do not get a
score and do not add complexity to their parent.

Poly-crap does not read JavaScript source maps. Coverage producers must report
the original TypeScript paths when TypeScript results are required.

## Development

The project uses Rust 1.88.0. Before submitting a change, run:

```sh
cargo fmt --all --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets
cargo package --locked --allow-dirty
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for contribution guidance,
[SECURITY.md](SECURITY.md) for vulnerability reporting, and
[RELEASING.md](RELEASING.md) for maintainer release steps. See
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) for cargo-crap attribution.

## License

MIT
