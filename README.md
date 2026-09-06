# poly-crap

[![CI](https://img.shields.io/github/actions/workflow/status/drew-simmons/poly-crap/ci.yml?branch=main&label=CI)](https://github.com/drew-simmons/poly-crap/actions/workflows/ci.yml)
[![Latest release](https://img.shields.io/github/v/release/drew-simmons/poly-crap?label=release)](https://github.com/drew-simmons/poly-crap/releases/latest)
[![Docs](https://img.shields.io/badge/docs-drew--simmons.github.io-blue)](https://drew-simmons.github.io/poly-crap/)
[![License](https://img.shields.io/github/license/drew-simmons/poly-crap)](LICENSE)

Poly-crap finds the functions in a codebase that are both complex and poorly
tested. It parses your source with tree-sitter, reads the coverage report your
test runner already writes, and scores every function with the CRAP metric.
It supports TypeScript, JavaScript, Python, Go, Rust, and Java, and it ships
as one binary with no runtime dependencies.

Use it three ways:

- In a pull request, to fail the build when a changed function is too complex
  for its tests. See
  [Changed functions](https://drew-simmons.github.io/poly-crap/gating/changed-functions).
- Against a baseline, to fail the build when any score gets worse. See
  [Baselines](https://drew-simmons.github.io/poly-crap/gating/baselines).
- On a whole repository, to find the functions most worth testing or splitting
  next.

## Install

### Installer (macOS and Linux)

```sh
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/drew-simmons/poly-crap/releases/latest/download/poly-crap-installer.sh \
  | sh
```

The installer downloads a prebuilt release, checks its SHA-256 checksum, and
puts `poly-crap` in `~/.cargo/bin`. To pin a version, replace
`latest/download` with `download/v0.8.0`.

### Cargo

With Rust 1.88 or newer:

```sh
cargo install poly-crap --locked
```

Or run `cargo install --path . --locked` from a clone. Prebuilt binaries cover
macOS and Linux, and there is no Windows build; see
[Install](https://drew-simmons.github.io/poly-crap/install) for the targets.

## Quick start

1. Run your tests with coverage on. Poly-crap reads the report; it never runs
   tests itself.
   [Coverage reports](https://drew-simmons.github.io/poly-crap/coverage) lists
   the command for each stack.

2. Run poly-crap from the repository root. It finds a report at any of the
   default locations on its own:

   ```sh
   poly-crap
   ```

3. Read the table. Each row is a function over the threshold, and the last
   column names the lines a test never reached:

   ```text
   CRAP results
     CRAP   CC  Coverage  Language  Symbol  Location    Uncovered
      6.0  4.0     50.0%  python    run     ./app.py:4  6-8
     1 more function not shown; adjust --min or --top to list them.
   1 of 2 functions exceeds CRAP threshold 5.0.
   Coverage scope: 1 analyzed file, 1 coverage source file, 1 matched, 0 source-only, 0 coverage-only.
   ```

4. Gate a branch. This scores only the functions the branch changed and exits
   `1` when one is over the threshold:

   ```sh
   poly-crap --diff-base main --fail-above
   ```

## How the score works

```text
CRAP(m) = complexity(m)² × (1 - coverage(m) / 100)³ + complexity(m)
```

`complexity` is the function's cyclomatic complexity and `coverage` is the
share of its executable lines that a test ran. At 100% coverage the score
equals the complexity, so a threshold also caps complexity. The coverage term
is cubed, so the first tests on a function pay off the most.

The default threshold is `5`, which suits a small, well-tested codebase that
wants to stay that way. The metric's authors proposed `30`, which catches the
worst offenders in an existing codebase first.
[How the score works](https://drew-simmons.github.io/poly-crap/how-the-score-works)
explains both and how to pick one.

## Documentation

The full manual is at <https://drew-simmons.github.io/poly-crap/>:

- [Coverage reports](https://drew-simmons.github.io/poly-crap/coverage):
  formats, producing a report, auto-discovery, and path matching.
- [Reading the output](https://drew-simmons.github.io/poly-crap/output): the
  table, color, JSON, and SARIF.
- [Gating changes](https://drew-simmons.github.io/poly-crap/gating/changed-functions):
  changed functions, baselines, and CI recipes.
- [What gets scored](https://drew-simmons.github.io/poly-crap/scope):
  languages, functions, complexity rules, excludes, and allow lists.
- [Reference](https://drew-simmons.github.io/poly-crap/reference/options):
  every option, the configuration file, and exit codes.
- [Troubleshooting](https://drew-simmons.github.io/poly-crap/troubleshooting).

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

The docs site lives in `docs/`. Run `pnpm --dir docs install` once, then
`pnpm --dir docs run dev` to serve it locally.

See [CONTRIBUTING.md](CONTRIBUTING.md) for contribution guidance,
[SECURITY.md](SECURITY.md) for vulnerability reporting, and
[RELEASING.md](RELEASING.md) for maintainer release steps. See
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) for cargo-crap attribution.

## License

MIT
