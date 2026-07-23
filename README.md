# publish-crates

[<img alt="build status" src="https://img.shields.io/github/actions/workflow/status/romnn/publish-crates/build.yaml?label=build">](https://github.com/romnn/publish-crates/actions/workflows/build.yaml)
[<img alt="test status" src="https://img.shields.io/github/actions/workflow/status/romnn/publish-crates/test.yaml?label=test">](https://github.com/romnn/publish-crates/actions/workflows/test.yaml)
[![dependency status](https://deps.rs/repo/github/romnn/publish-crates/status.svg)](https://deps.rs/repo/github/romnn/publish-crates)
[<img alt="docs.rs" src="https://img.shields.io/docsrs/publish-crates/latest?label=docs.rs">](https://docs.rs/publish-crates)
[<img alt="crates.io" src="https://img.shields.io/crates/v/publish-crates">](https://crates.io/crates/publish-crates)

`publish-crates` publishes Cargo workspace packages in dependency order. Independent packages can
publish concurrently; a package with local path dependencies waits until those dependencies are
available on crates.io.

The repository provides:

- `cargo-publish-crates`, a Cargo subcommand;
- `publish-crates`, the reusable Rust library;
- a composite GitHub Action.

## Installation

```bash
brew install --cask romnn/tap/cargo-publish-crates

# Or install from crates.io.
cargo install --locked cargo-publish-crates
```

## Command-line usage

Run against the current workspace:

```bash
cargo publish-crates --dry-run
```

Select packages by Cargo package name. Exclusions take precedence over inclusions:

```bash
cargo publish-crates \
  --path ./my-workspace \
  --include core \
  --include cli \
  --exclude internal-tools
```

Arguments after `--` are forwarded to every `cargo publish` invocation:

```bash
cargo publish-crates -- --registry private
```

The corresponding environment variables use the `PUBLISH_CRATES_` prefix, such as
`PUBLISH_CRATES_DRY_RUN`, `PUBLISH_CRATES_REGISTRY_TOKEN`, and
`PUBLISH_CRATES_CONCURRENCY_LIMIT`.

## Resolving workspace versions

Cargo requires published path dependencies to carry a version requirement. Pass
`--resolve-versions` to replace local path dependency requirements with exact versions from their
workspace packages:

```toml
# Before
publish-crates = { path = "../publish-crates" }

# After
publish-crates = { path = "../publish-crates", version = "=0.0.29" }
```

This updates package manifests and path-only entries in `[workspace.dependencies]`. Combine it with
`--dry-run` to inspect the dependency graph without modifying manifests; packages with unresolved
local dependencies cannot run Cargo's own dry-run until those versions are written.

## GitHub Action

```yaml
- uses: romnn/publish-crates@v0.0.29
  with:
    registry-token: ${{ secrets.CARGO_REGISTRY_TOKEN }}
    resolve-versions: true
    include: core,cli
    exclude: internal-tools
    concurrency-limit: 4
    max-retries: 5
```

`include` and `exclude` accept comma- or whitespace-separated package names. `extra-args` accepts a
shell-quoted argument string and forwards each parsed argument to `cargo publish`.

## Library

The [`publish-crates` API documentation](https://docs.rs/publish-crates) describes the option
semantics, manifest mutation behavior, defaults, and failure conditions.

## Development

Run the cargo plugin locally:

```bash
cargo run -p cargo-publish-crates -- --path ./path/to/crate
```

Run the same checks used by CI:

```bash
task lint:fc
task test:fc
task test:doc
task spellcheck
```
