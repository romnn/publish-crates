## publish crates

[<img alt="build status" src="https://img.shields.io/github/actions/workflow/status/romnn/publish-crates/build.yml?label=build">](https://github.com/romnn/publish-crates/actions/workflows/build.yml)
[<img alt="test status" src="https://img.shields.io/github/actions/workflow/status/romnn/publish-crates/test.yml?label=test">](https://github.com/romnn/publish-crates/actions/workflows/test.yml)
[<img alt="crates.io" src="https://img.shields.io/crates/v/publish-crates">](https://crates.io/crates/publish-crates)

#### TODO
- dry-run and offline dont work together, we should manually allow this case where the version cannot be found on crates.io
- implement fallback to "latest" version
- display all paths releative
- stream the output of async subcommands? when multiple are running that will be an issue though...

#### Development
```bash
yarn upgrade action-get-release --latest
```
