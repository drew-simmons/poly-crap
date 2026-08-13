# Contributing

Thank you for helping improve `poly-crap`.
Use a GitHub issue to report a bug or suggest a change. Open a pull request when
you have a tested fix.

## Development setup

1. Install Rust 1.88.0 using `rustup` or `mise`.
2. Install prek 0.4.12:

   ```sh
   uv tool install prek==0.4.12
   ```

3. Clone the repository:

   ```sh
   git clone https://github.com/drew-simmons/poly-crap.git
   cd poly-crap
   ```

4. Install the Git hook:

   ```sh
   prek install
   ```

5. Run the hooks and build the crate package:

   ```sh
   prek run --all-files
   cargo package --locked --allow-dirty
   ```

The hook runs repository checks, `cargo fmt`, Clippy with warnings denied, and
the test suite. The matching Rust commands are:

```sh
cargo fmt --all --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets
```

> [!IMPORTANT]
> Tests must not require network access, private credentials, or changes to a
> developer's system. Keep outside process and network work in small units that
> tests can replace.

## Commit messages

Use Conventional Commit subjects because release automation derives versions
and release notes from commits merged to `main`:

```text
fix: handle an empty input file
feat: add JSON output
feat!: change the command output
```

Use `fix:` for bug fixes, `feat:` for features, and `!` or a
`BREAKING CHANGE:` footer for incompatible changes. Types such as `docs:`,
`test:`, `ci:`, and `chore:` do not trigger a release by themselves.

> [!TIP]
> The pull request title should follow this format. The project uses squash
> merges, so that title becomes the commit subject on `main`.

## Pull requests

- Keep changes focused and explain their user-visible effect.
- Add tests for new behavior and fixed bugs.
- Update the docs when commands, options, or requirements change.
- Do not commit credentials, generated build output, or local tool state.

Before you open a pull request, run both checks from the setup steps and
describe any check you could not run.

By contributing, you agree that your contributions are licensed under the MIT
License.
