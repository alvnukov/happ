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

### Linux packages from Releases

Download artifacts from the latest [GitHub Releases](https://github.com/alvnukov/happ/releases):

- `.deb` packages: `happ_<version>_amd64.deb`, `happ_<version>_arm64.deb`
- `.rpm` packages: `happ-<version>-1.x86_64.rpm`, `happ-<version>-1.aarch64.rpm`

### Windows installer from Releases

Download `happ_windows_amd64_installer.exe` from the latest [GitHub Releases](https://github.com/alvnukov/happ/releases) and run it.

Installer target path: `C:\Program Files\happ\happ.exe`
The installer enables `Add happ to PATH` by default (for next-next-next setup).

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
happ completion zsh
```

```bash
# write to file
happ completion bash --output /tmp/happ.bash
```

Compatibility: `happ completion --shell zsh` also works.

Quick one-liner for current shell session (similar to `kubectl`):

```bash
# zsh
source <(happ completion zsh)

# bash
source <(happ completion bash)
```

### Configure completion in your shell

#### zsh

```bash
mkdir -p "${HOME}/.zsh/completions"
happ completion zsh --output "${HOME}/.zsh/completions/_happ"
```

Add to `~/.zshrc`:

```bash
fpath=("${HOME}/.zsh/completions" $fpath)
autoload -Uz compinit && compinit
```

Reload shell:

```bash
exec zsh
```

#### bash

```bash
mkdir -p "${HOME}/.local/share/bash-completion/completions"
happ completion bash --output "${HOME}/.local/share/bash-completion/completions/happ"
```

Reload shell:

```bash
exec bash
```

#### fish

```bash
mkdir -p "${HOME}/.config/fish/completions"
happ completion fish --output "${HOME}/.config/fish/completions/happ.fish"
```

Reload shell:

```bash
exec fish
```

#### PowerShell

```powershell
$dir = Split-Path -Parent $PROFILE
New-Item -ItemType Directory -Force -Path $dir | Out-Null
happ completion powershell | Out-String | Add-Content -Path $PROFILE
```

Restart PowerShell session.

#### elvish

```bash
mkdir -p "${HOME}/.config/elvish/lib"
happ completion elvish --output "${HOME}/.config/elvish/lib/happ.elv"
```

Add to `~/.config/elvish/rc.elv`:

```elvish
use happ
```

## Web mode

```bash
# web mode for tests/CI without opening browser
happ --web --web-open-browser=false
```

## LSP mode (experimental)

`happ` provides an experimental Language Server entrypoint:

```bash
happ lsp --stdio=true
```

Current status:

- LSP handshake and lifecycle (`initialize` / `shutdown` / `exit`) are implemented.
- Incremental document state and publish diagnostics are implemented for helm-apps include checks.
- Custom method `happ/resolveEntity` is available (server-side include/env resolution payload for IDE features).
- Full helm-apps language feature parity is still in progress (`experimental.helmAppsFullLanguageFeatures=false`).

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
- default ref: `helm-apps-1.8.4`
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

## Linting

Run Rust linters:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets --locked
```

Run web asset linter:

```bash
cd web
npm ci
npm run lint
```
