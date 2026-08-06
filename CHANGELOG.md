# Changelog

All notable changes to this project will be documented in this file.

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

