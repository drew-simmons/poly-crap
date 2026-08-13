# poly-crap

[![CI](https://img.shields.io/github/actions/workflow/status/drew-simmons/poly-crap/ci.yml?branch=main&label=CI)](https://github.com/drew-simmons/poly-crap/actions/workflows/ci.yml)
[![Latest release](https://img.shields.io/github/v/release/drew-simmons/poly-crap?label=release)](https://github.com/drew-simmons/poly-crap/releases/latest)
[![License](https://img.shields.io/github/license/drew-simmons/poly-crap)](LICENSE)

A fast, focused command-line tool written in Rust.

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

```text
poly-crap [OPTIONS]

Options:
  -h, --help     Print help
  -V, --version  Print version
```

The generated command provides a tested `clap` help and version scaffold. Add
the project's commands and options to the `Cli` type in `src/main.rs`.

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
[RELEASING.md](RELEASING.md) for maintainer release steps.

## License

MIT
