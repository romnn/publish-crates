## publish crates

[<img alt="test status" src="https://img.shields.io/github/actions/workflow/status/romnn/publish-crates/test.yml?label=test">](https://github.com/romnn/publish-crates/actions/workflows/test.yml)
[<img alt="crates.io" src="https://img.shields.io/crates/v/cargo-publish-crates">](https://crates.io/crates/cargo-publish-crates)

#### TODO

- dry-run and offline dont work together, we should manually allow this case where the version cannot be found on crates.io
- implement fallback to "latest" version
- display all paths releative
- stream the output of async subcommands? when multiple are running that will be an issue though...

#### Development

Run the cargo plugin locally:

```bash
cargo run -p cargo-publish-crates -- --path ./path/to/crate
```

Update the github action:

```bash
yarn upgrade action-get-release --latest
```
