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

### Development quality checks

```bash
# formatting check
cargo fmt-check

# strict safety-oriented lint profile for production targets
cargo lint

# advisory lint pass for tests
cargo lint-tests
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

Web mode is a single-user local tool, and the server enforces that:

- `Host` must be a loopback address, so a hostname that resolves to `127.0.0.1`
  cannot reach it;
- `Origin`, when the client sends one, must be loopback too, so a page on another
  site cannot drive the API through the browser;
- `POST /api/*` requires `Content-Type: application/json`, which makes every API
  call a preflighted request that a foreign page cannot send;
- `/exit` is `POST` only, so it cannot be triggered by an image tag or a prefetch.

Local clients keep working unchanged: `curl` without an `Origin` header is accepted,
and the UI itself already sends JSON on every call.

## UI development

`happ` web UI is server-rendered from Rust, so the local dev loop is:

```bash
# first run builds CodeMirror bundle, compiles happ, then starts happ --web
./scripts/ui-dev.sh
```

Useful variants:

```bash
# keep server local but do not auto-open browser
./scripts/ui-dev.sh --no-browser

# reuse already running happ --web and run the full Playwright suite
./scripts/ui-check.sh --reuse-running

# visual-only pass
./scripts/ui-check.sh --visual

# refresh visual baselines
./scripts/ui-check.sh --visual --update-snapshots

# headed Playwright run against an already running server
(cd web && HAPP_WEB_BASE_URL=http://127.0.0.1:18088 HAPP_WEB_SKIP_SERVER=1 \
  npm run test:ui:headed)
```

Artifacts you can inspect after UI checks:

- Playwright HTML report: `web/test-results/playwright-report-html/index.html`
- visual baselines: `web/tests/visual.spec.mjs-snapshots/*.png`

## Studio mode (no port)

```bash
# studio backend over stdio (no HTTP listener)
happ --studio
```

## MCP server mode

`happ mcp --stdio` runs happ as a [Model Context Protocol](https://modelcontextprotocol.io)
server, so an AI client can ask about a chart instead of shelling out and
re-parsing CLI output.

```bash
happ mcp --stdio
```

### Register it in your client

```bash
# project-scoped, in the current directory
happ mcp setup -c claude,codex,opencode

# user-wide instead
happ mcp setup -c claude --global

# see what would change, change nothing
happ mcp setup -c codex --dry-run
```

Setup writes three things per client, because registering the server only makes
the tools *available* — what decides whether a model reaches for them is the
project's instruction file, and what carries the workflows without sitting in
context all session is a skill:

| | claude | codex | opencode |
|---|---|---|---|
| server entry | `.mcp.json` / `~/.claude.json` | `~/.codex/config.toml` | `opencode.json` / `~/.config/opencode/opencode.json` |
| instructions | `CLAUDE.md` / `~/.claude/CLAUDE.md` | `AGENTS.md` / `~/.codex/AGENTS.md` | `AGENTS.md` / `~/.config/opencode/AGENTS.md` |
| skills | `.claude/skills/` | — | `.opencode/skills/` |

Claude Code reads `CLAUDE.md` and not `AGENTS.md`, which is why the two are not
interchangeable here. Codex documents no skill directory, so happ installs none
for it rather than guessing at a path it would never read. The skills themselves
— `happ-charts` and `happ-code` — are plain [Agent Skills](https://agentskills.io)
`SKILL.md` files you can read and edit.

Use `--no-instructions` or `--no-skills` to install less than all three.

**Nothing is clobbered and nothing is written twice.** Other servers in a config
survive, and so does everything you wrote in `CLAUDE.md` or `AGENTS.md`: happ
keeps its own text between `<!-- happ:start -->` and `<!-- happ:end -->` markers
and only ever replaces what is between them, in place. Every write is compared
against the file first, so a second `setup` reports that everything is already
current and does not touch a single file — not the bytes, not the mtime.

To undo it, `remove` takes the same clients and flags:

```bash
happ mcp remove -c claude,codex,opencode   # or: happ mcp uninstall
```

It takes out the `happ` entry, happ's block, and happ's skill files — and
nothing else. Sections and directories left empty go with them, and a config
that would be left holding only `{}` is removed rather than left as a husk, so
`setup` followed by `remove` returns the tree to what it was. Anything happ was
never registered in is reported as such and left byte for byte as it was.

Pin a default chart so tool calls can omit the path:

```bash
happ mcp --stdio --chart ./charts/my-app
```

### Tools

Two tools, each taking an `op`. The surface is deliberately small: every tool
description sits in the model's context on every request.

`helm_apps` — a chart built on the helm-apps library:

| op | needs | answers |
|----|-------|---------|
| `overview` | — | groups, apps, environments, library version, violation counts |
| `apps` | — | every app as `group.app` with its enabled state |
| `resolve` | `group`, `app` | the app's values after include expansion and env selection |
| `render` | `group`, `app` | the Kubernetes manifests the app produces |
| `lint` | — | violations of the helm-apps contract |
| `diff` | `group`, `app`, `from_env`, `to_env` | how the app differs between two environments |
| `query` | `query` | jq over the whole resolved values tree |
| `contract` | — | the library's own rules: groups, functions, env selection |
| `template` | `name` | source of a library template or `define` |

A chart is judged by the values it is deployed with, so every op above also
takes the values the deployment layers on top:

| argument | means |
|----------|-------|
| `values_files` | extra values files, in order, the way `helm -f` does. Relative paths resolve against the chart directory. |
| `set` | overrides by dotted path, the way `helm --set` does — `global.vars.HOST`, `hosts[0].name`, `config\.yaml` for a dot inside a key. Applied after `values_files`. |
| `set_string` | the same, but every value stays a string. |

They are applied before `_include` and `_includeFile` expansion, which is where
Helm applies them too, so `resolve`, `lint` and `query` describe the same chart
`render` produces rather than the one values.yaml describes on its own.

`op=lint` checks the chart against the library contract, including mistakes that
otherwise surface only as an unreadable Go template trace:

- `E_INCLUDE_NOT_A_LIST` — `_include: name` written as a scalar. The library
  ranges over it, so the render fails with `range can't iterate over name`.
- `E_LIBRARY_NOT_INITIALISED` — the chart declares `apps-*` groups but no
  template calls `apps-utils.init-library`, so `helm template` silently renders
  nothing at all.
- `E_UNKNOWN_APPS_GROUP`, `E_UNRESOLVED_INCLUDE`, `E_INCLUDE_FILE_NOT_FOUND`,
  `E_ENV_REGEX_AMBIGUOUS`, plus Go template syntax errors.

`op=resolve` carries a warning when the chart has an error that stops it
rendering, because happ resolves values more leniently than the library renders
them. `op=render` leads with the cause of a failure rather than the template
trace, says when an app is disabled for the environment it was asked about, and
notes that the default `fast` renderer is an in-process approximation.

`code` — code intelligence backed by real language servers:

| op | needs | answers |
|----|-------|---------|
| `languages` | — | which languages are available, installed and running |
| `diagnostics` | `file` | errors and warnings |
| `definition` | `file`, `symbol` | where a symbol is defined |
| `references` | `file`, `symbol` | every use of a symbol |
| `hover` | `file`, `symbol` | type, signature and docs |
| `symbols` | `file` — or `query` alone | symbols in a file, or a project-wide search |
| `calls` | `file`, `symbol` | callers, or callees with `direction=outgoing` |

Symbols may be addressed by name or by one-based `line` and `column`. A name
need only *appear* in `file`: asking about a function that is called there but
declared elsewhere is how you find out where it comes from, and the answer says
which position it was resolved from. A project-wide `symbols` search needs no
`file` at all — the project is the working directory.

Positions come back relative to that directory when they are inside it, and can
be passed straight back in.

### Language servers

happ answers helm-apps values files itself, in-process, using the same code
path as `happ lsp`. Other languages are served by that ecosystem's own language
server, started on demand and kept warm:

| language | server | selected by |
|----------|--------|-------------|
| helm-apps | built into happ | `values.yaml` inside a chart |
| Go | `gopls` | `.go` |
| Rust | `rust-analyzer` | `.rs` |
| TypeScript / JavaScript | `typescript-language-server` | `.ts` `.tsx` `.js` `.jsx` … |
| Python | `pyright-langserver` | `.py` `.pyi` |
| C / C++ | `clangd` | `.c` `.h` `.cc` `.cpp` … |

Servers are found on `PATH` and in each toolchain's default install location
(`$GOPATH/bin`, `~/.cargo/bin`, …), because a client started from a desktop
session often has neither on its `PATH`.

The first call for a project waits for the server to finish loading it —
seconds for a small module, longer for a large workspace — and every later call
is instant. Readiness comes from what each server documents: rust-analyzer's
[`experimental/serverStatus`](https://rust-analyzer.github.io/book/contributing/lsp-extensions.html)
with `quiescent: true`, and the LSP `$/progress` cycle for gopls and the rest.
Without that wait a cold server answers instantly and emptily, so a missing
symbol and an unfinished index look identical. When a server does answer
emptily, what it reported about its own health is included, so "no symbols here"
is never confused with "I could not load this project" — but deduplicated and
capped first, and taken from `window/logMessage` rather than raw stderr whenever
the server used it, because rust-analyzer will otherwise answer one clean file
with seventy copies of one internal log line.

`code` with `op=languages` reports which are installed. Override any of them:

```bash
happ mcp --stdio --language-server go=/opt/bin/gopls --language-server python=pylsp
```

Adding a language is one entry in `src/mcp/bridge/registry.rs`; no tool or
schema changes.

### Resources

The embedded helm-apps chart is served as `happ://helm-apps/...` resources, so a
model can read the library's own template source rather than recall it.

### Developing the server

A client spawns a stdio MCP server once and never again, so every recompile
normally costs a client restart — and for an agent, the conversation with it.
`scripts/happ-mcp-dev.mjs` is a harness that removes that cost: it is the server
the client connects to, and it runs the real `happ mcp --stdio` as a child it can
replace underneath a live session.

```jsonc
// .mcp.json
{ "mcpServers": { "happ": { "command": "node", "args": ["/abs/path/to/scripts/happ-mcp-dev.mjs"] } } }
```

It proxies every request through unchanged and adds one tool, `happ_dev`:

| op | does |
|----|------|
| `status` | whether the child is up, which binary and arguments, when it was built |
| `rebuild` | `cargo build`, then restart on the new binary |
| `restart` | restart on the current binary, e.g. after an external `cargo build` |
| `stop` | stop the child; the next tool call starts it again |
| `logs` | the child's stderr and the harness's own event log |
| `config` | show or change profile, toolchain, `--chart`, language servers, timeout |
| `check` | `cargo fmt --check`, `cargo lint` and the mcp tests, reported together |
| `raw` | send one JSON-RPC method to the child verbatim, for testing the protocol layer |

After a change to the tool catalog the harness sends
`notifications/tools/list_changed`, so the client re-reads it without
reconnecting. A failed build leaves the previous binary running and returns the
compiler output, and a child that will not start at all is reported through
`happ_dev` rather than taking the session down with it. State lives in
`.tmp/mcp-dev/`.

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
- default ref: `helm-apps-1.9.0`
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
