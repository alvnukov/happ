# Changelog

## [1.2.1] - 2026-07-30

### Added

- Added fleet-wide `helm_apps op=query_manifests`, with `{group, app, manifest}` provenance and server-side `kind`/`resource` filtering before jq.
- Added value-origin reporting and focused per-resource manifest rendering for helm-apps applications.

### Changed

- Reduced repeated chart loading across MCP analysis operations and made truncated render responses point callers to named resources instead of larger output limits.

### Fixed

- Aligned fast, Helm and werf renders on the same assembled values root, release identity and environment behavior.
- Improved diagnostics for invalid app names, unresolved includes, environment maps, query mistakes and incomplete container images.

## [1.2.0] - 2026-07-29

### Added

- Added `happ mcp`, which serves happ's helm-apps chart analysis and language-server-backed code analysis to an LLM over the Model Context Protocol via stdio.
- Added `happ mcp setup` and `happ mcp remove` to register and unregister the server with claude, codex and opencode, project-scoped or user-wide, both supporting `--dry-run`.
- Published the embedded helm-apps chart as MCP resources so a client can read the library contract verbatim.
- Added `--parent-pid` to the MCP server so it exits with a host that dies without closing the pipe.

### Changed

- Extracted the analysis the LSP performed inline into shared `analysis_*` entry points that both the LSP and the MCP tools call, so the two front ends cannot drift.
- `dyfflike` now produces structured changes carrying both sides of a change, not just rendered text.

### Fixed

- Web mode now serves local requests only: `Host` and `Origin` must be loopback, `POST /api/*` requires `Content-Type: application/json`, and `/exit` is `POST` only. This closes cross-origin writes through `/api/save-chart`, cross-origin reads through `/api/fs-list`, DNS rebinding, and shutdown from an `<img>` tag.
- Web mode no longer mistakes the socket read timeout for the end of a request body, so a client that pauses between headers and body is no longer rejected with a JSON parse error.

## [1.1.7] - 2026-03-15

### Added

- Added `happ`-backed manifest preview renderers for faster preview flows inside the editor.

### Changed

- Improved `happ` library CLI and diagnostics so preview and library workflows expose clearer runtime errors.
- Switched fast manifest preview rendering to the raw helper render path.
- Aligned fast preview manifest rendering with the main render pipeline for closer output parity.

### Fixed

- Preserved sibling applications from the selected group in manifest preview values while still forcing the selected entity `enabled: true`.
- Kept the Homebrew build formula wired to the Go helper build dependency required by `happ`.
