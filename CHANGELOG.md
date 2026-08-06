# Changelog

All notable changes to this project will be documented in this file.

## [0.1.2] - 2026-08-06

### Changed

- Refined area sparkline rendering in `index.html` for higher contrast on both normal and single-point history states.
- Increased visual distinction between area fill, stroke, and baseline for better trend recognition at small scale.

### Docs

- Added changelog note reflecting the latest visualization behavior.

## [0.1.1] - 2026-08-06

### Changed

- Updated dashboard sparkline rendering from thin line to gradient-filled area sparkline in `index.html`.
- Improved small-screen trend readability with stronger strokes and a subtle baseline.

### Docs

- Updated `README.md` to document the UI trend chart behavior.

## [0.1.0] - 2026-08-06

### Added

- Initial release of `aidash`.
- Built Rust + Axum backend with background polling loop.
- Added providers:
  - z.ai usage collection
  - Claude usage collection
  - Codex usage collection
- Added dashboard UI at `/` from embedded `index.html`.
- Added JSON APIs:
  - `GET /data.json`
  - `GET /history.json`
- Added tmux-based control script `ctl.sh`.
- Added repository documentation and ignore rules for build artifacts/keys.

### Changed

- Added startup bootstrap collection before server begins accepting requests.

### Notes

- Codex refresh flow reads from auth file and attempts token refresh when needed.
- Default polling interval is 300 seconds unless overridden by `AIDASH_INTERVAL`.
