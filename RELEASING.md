# Releasing

Release Please manages versions, `Cargo.lock`, `CHANGELOG.md`, tags, and GitHub
release notes from Conventional Commits. Cargo-dist 0.32.0 builds the macOS and
Linux archives, checksums, and shell installer for each release tag.

## One-time GitHub setup

1. Optional: to let Release Please pull requests start CI without manual
   approval, create a fine-grained personal access token scoped only to
   `drew-simmons/poly-crap` with these repository permissions:

   - Contents: read and write
   - Issues: read and write
   - Pull requests: read and write

2. Add that token as the repository Actions secret `RELEASE_PLEASE_TOKEN`.
   Release builds do not need this token.
3. In **Settings → Actions → General**, enable **Allow GitHub Actions to create
   and approve pull requests**. The `GITHUB_TOKEN` fallback requires it.
4. Create a crates.io API token and add it as the repository Actions secret
   `CARGO_REGISTRY_TOKEN`.
5. In **Settings → General → Pull Requests**:

   - Disable merge commits.
   - Enable squash merging.
   - Set the default squash commit message to **Pull request title**.
   - Disable rebase merging.
   - Enable automatic deletion of head branches.

6. After the workflows have run for a pull request, create an active ruleset in
   **Settings → Rules → Rulesets** that targets the default branch:

   - Restrict branch deletion.
   - Block force pushes.
   - Require a pull request. Zero approvals is suitable for a solo maintainer.
   - Require linear history.
   - Require these status checks:
     - `Validate PR title`
     - `Format, lint, and package`
     - `Test (ubuntu-24.04)`
     - `Test (macos-14)`
     - `plan`

> [!IMPORTANT]
> GitHub does not start tag workflows for tags created with the default
> `GITHUB_TOKEN`. After Release Please creates a tag, its workflow dispatches
> the cargo-dist workflow with `GITHUB_TOKEN`, which GitHub does allow. A
> dedicated token remains useful for CI on Release Please pull requests.

## Conventional Commits

Use Conventional Commit subjects on commits merged to `main`:

- `fix: ...` creates a patch release.
- `feat: ...` creates a minor release.
- `feat!: ...` or a `BREAKING CHANGE:` footer creates a breaking release.
- `docs:`, `test:`, `ci:`, and `chore:` do not create releases by themselves.

Before version 1.0, breaking changes bump the minor version. Release Please
keeps implementation-only commit types out of the public changelog.

## Automated release flow

1. Push or merge Conventional Commits to `main`.
2. Release Please creates or updates a Release PR with the next version,
   `Cargo.toml`, `Cargo.lock`, and `CHANGELOG.md` changes.
3. Review its notes and merge the Release PR after CI passes.
4. Release Please creates the version tag and a draft GitHub Release.
5. Release Please starts cargo-dist for the tag. After all targets build,
   cargo-dist attaches the archives, checksums, and `poly-crap-installer.sh`.
   It then publishes the draft release.
6. The release workflow publishes the same version to crates.io.

The repository starts at version `0.0.0`. Use `feat: initial release` for the
first project commit so the first Release PR proposes `v0.1.0`.

> [!IMPORTANT]
> Do not bump the package version, edit generated changelog entries, or create
> release tags by hand. If cargo-dist fails, fix the cause and rerun the failed
> workflow in GitHub Actions. The draft release stays unpublished.

If Release Please created a tag but cargo-dist did not start, run the
**Release** workflow from the Actions tab and enter the existing tag. This
publishes the draft without moving or recreating the tag.

> [!WARNING]
> `.github/workflows/release.yml` has custom steps that upload to the draft from
> Release Please and publish the crate to crates.io. If `dist generate` rewrites
> the workflow, restore the `Publish GitHub Release` and
> `Publish crate to crates.io` steps before you merge the change.

Do not move or recreate a published tag. Fix released defects with a new patch
release.
