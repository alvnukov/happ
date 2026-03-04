# happ

[![CI](https://github.com/alvnukov/happ/actions/workflows/ci.yml/badge.svg)](https://github.com/alvnukov/happ/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/alvnukov/happ?label=release)](https://github.com/alvnukov/happ/releases)
[![Homebrew](https://img.shields.io/badge/homebrew-alvnukov%2Ftap%2Fhapp-fbb040?logo=homebrew)](https://github.com/alvnukov/homebrew-tap)
[![Coverage](https://codecov.io/gh/alvnukov/happ/graph/badge.svg?branch=main)](https://codecov.io/gh/alvnukov/happ)

`happ` is a Rust CLI focused on import/inspect/diff/query workflows.

## Installation

### Homebrew (recommended)

```bash
brew tap alvnukov/tap
brew install alvnukov/tap/happ
```

### Build from source

```bash
git clone https://github.com/alvnukov/happ.git
cd happ
cargo build --release --locked
./target/release/happ --help
```

## Query commands

`happ jq` and `happ yq` now differ by query language style only.

- `happ jq`: jq-like syntax
- `happ yq`: yq-like syntax
- both commands accept **JSON and YAML** input (auto-detected)

### Examples

```bash
# jq syntax over YAML input
happ jq --query '.apps[] | .name' --input values.yaml
```

```bash
# yq syntax over JSON input
happ yq --query '.apps[] | .name' --input values.json
```

```bash
# stdin also supports both formats
cat values.yaml | happ jq --query '.global.env' --input -
cat values.json | happ yq --query '.global.env' --input -
```

Output options:

- `--compact`
- `--raw-output` (prints raw string values without JSON quotes)

## Shell completion

`happ` can generate completion scripts for:

- `bash`
- `zsh`
- `fish`
- `powershell`
- `elvish`

Examples:

```bash
# print to stdout
happ completion --shell zsh
```

```bash
# write to file
happ completion --shell bash --output /tmp/happ.bash
```

```bash
# web mode for tests/CI without opening browser
happ --web --web-open-browser=false
```

## Parity Matrix (CLI contracts)

Core CLI behavior is pinned by integration parity tests.

- test file: `tests/parity_cli.rs`
- fixtures: `tests/parity/fixtures/*`
- covered contracts:
  - `help`
  - `validate`
  - `jq`
  - `yq`
  - `dyff`
  - `manifests`
  - `compose`
  - `completion`
  - embedded `charts/helm-apps` asset generation

Run locally:

```bash
cargo test --test parity_cli
```

## Embedded library source

During build, `happ` fetches `helm-apps` chart from GitHub and embeds it into binary.

- default repo: `https://github.com/alvnukov/helm-apps.git`
- default ref: `main`
- override repo: `HELM_APPS_GITHUB_REPO`
- override ref: `HELM_APPS_GITHUB_REF`
- force local chart path: `HELM_APPS_CHART_PATH=/abs/path/to/charts/helm-apps`

## Test coverage

Coverage is calculated in CI in the `coverage` job (`cargo llvm-cov`) and uploaded to Codecov.

You can reproduce locally:

```bash
rustup component add llvm-tools-preview
cargo install cargo-llvm-cov
cargo llvm-cov --workspace --all-features --summary-only
```
