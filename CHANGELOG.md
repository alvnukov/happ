# Changelog

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
