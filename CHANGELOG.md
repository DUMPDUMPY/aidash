# Changelog

All notable changes to this project will be documented in this file.

## [1.0.1] - 2026-08-13

### Fixed

- Codex provider intermittently returned no data (~20-30% of snapshots). Root cause was stale keep-alive connections in the reqwest pool: after idling through the poll interval the server-side socket was closed, so reusing it made the body read fail. `r.text().await.unwrap_or_default()` then returned an empty string and surfaced as an opaque `bad json` error.
- `codex_get` now returns `(status, body)` with a single retry (2 s backoff) on transport error, so stale pooled connections are recovered transparently.
- Auth-refresh trigger now keyed off HTTP status (401/403) in addition to the parsed error field, and the error field reports the real HTTP status plus a body snippet instead of `bad json`.

### Changed

- Default `AIDASH_INTERVAL` lowered from 60 s to 120 s in `ctl.sh`.

## [1.0.0] - 2026-08-06

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
- Updated dashboard sparkline rendering from thin line to gradient-filled area sparkline in `index.html`.
- Improved small-screen trend readability with stronger strokes and a subtle baseline.
- Refined area sparkline rendering for higher contrast on both normal and single-point history states.
- Increased visual distinction between area fill, stroke, and baseline for better trend recognition on small scale.

### Docs

- Updated `README.md` to document the UI trend chart behavior.
- Added changelog note reflecting the latest visualization behavior.

### Notes

- Codex refresh flow reads from auth file and attempts token refresh when needed.
- Default polling interval is 300 seconds unless overridden by `AIDASH_INTERVAL`.
