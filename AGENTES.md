# AGENTES

## Scope

This repository is `aidash`, a Rust + Axum dashboard that polls multiple AI usage endpoints and exposes a web UI plus JSON APIs.

## Purpose

- Aggregate usage data from z.ai, Claude, Codex, and Antigravity.
- Keep data in memory and provide:
  - latest snapshot at `/data.json`
  - short history at `/history.json`
- Serve dashboard UI at `/`.

## Directory map

- `src/main.rs` : server, collectors, background loop, API handlers.
- `index.html` : frontend UI loaded by `/`.
- `ctl.sh` : runtime control helper (tmux wrapper).
- `Cargo.toml` : dependencies and project metadata.
- `README.md` : public quick-start and runtime documentation.
- `AGENTES.md` : developer documentation.
- `CHANGELOG.md` : release notes and change log.

## Environment variables

Set these before running.

- `ZAI_KEY_FILE` (default `$HOME/aidash/zai.key`)
- `CLAUDE_CRED_FILE` (default `$HOME/.claude/.credentials.json`)
- `CODEX_AUTH_FILE` (default `$HOME/.codex/auth.json`)
- `ANTIGRAVITY_TOKEN_FILE` (default `$HOME/.gemini/antigravity-cli/antigravity-oauth-token`)
- `AIDASH_LISTEN` (default `0.0.0.0:8000`)
- `AIDASH_INTERVAL` (default `300`, seconds)
- `CODEX_DISABLE_REFRESH` (optional, set `1` to skip token refresh path)

## Runtime

- Binary: `target/release/aidash`
- Local start command:
  - `./target/release/aidash`
- Default behavior:
  - collect once immediately
  - spawn background loop for periodic refresh
  - history is capped by an internal limit of 60 snapshots

## Developer command style

- Use shell command prefix `rtk` in this environment (per repository instruction).
- Build:
  - `rtk cargo build --release`
- Run directly:
  - `rtk ./target/release/aidash`
- Run with manager script:
  - `rtk ./ctl.sh start`
  - `rtk ./ctl.sh stop`
  - `rtk ./ctl.sh status`
  - `rtk ./ctl.sh logs`
- Log file:
  - `aidash.log`

## Data model notes

- All provider objects contain:
  - `provider`
  - `status`
  - optional `plan`, `email`
  - `limits` array
- Each limit object should include:
  - `label`
  - `used_percent`
  - `remaining_percent`
  - optional `resets_at`
- Antigravity limit objects additionally carry:
  - `models`: list of model IDs in the quota pool
  - rows with `"unknown": true` and `used_percent: null` mean the API returned no quota info
  - a row labeled `... — limit hit` means the pool is exhausted (5h window)
- `any_error` in `/data.json` indicates any provider returned non-`ok`.

## Safety and secrets

- Do not commit credential files.
- Keep credentials local to trusted hosts.
- Treat parser failures as expected behavior when upstream API changes and update collectors accordingly.

## Non-goals

- No persistent DB.
- No auth layer in front of dashboard endpoint.
- No queueing system or long-term archival.

## Change process

- Update `CHANGELOG.md` for user-facing behavior changes.
- Keep code changes minimal and targeted.
- Prefer explicit error messages for credential/API response parsing issues.

