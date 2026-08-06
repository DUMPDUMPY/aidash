# AI Usage Dashboard (`aidash`)

An on-machine dashboard that pulls usage/credit usage data from multiple AI providers and visualizes it in one web page.

- Backend: `Rust + Axum`
- Frontend: `index.html` embedded in binary via `include_str!`
- Storage: in-memory snapshot buffer (no database)

---

## Features

- Single-page dashboard at `/`
- Provider usage graphs and status cards
- API endpoints:
  - `GET /data.json` for latest snapshot
  - `GET /history.json` for historical snapshots used by sparkline charts
- First snapshot is collected on startup, then refreshed by a background loop
- `tmux` control script (`ctl.sh`) for convenient process control

---

## Project structure

- `src/main.rs`: Axum server + provider collectors
- `index.html`: dashboard UI
- `ctl.sh`: control helper (`start|stop|restart|status|attach|logs`)
- `Cargo.toml`: Rust dependencies
- `.gitignore`: excludes build output and key files

---

## Build

```bash
cargo build --release
```

Binary output: `target/release/aidash`

---

## Run directly

```bash
export AIDASH_LISTEN=0.0.0.0:8000
export AIDASH_INTERVAL=300
./target/release/aidash
```

Defaults in code:

- `AIDASH_LISTEN`: `0.0.0.0:8000`
- `AIDASH_INTERVAL`: `300` (seconds)
- `ZAI_KEY_FILE`: `$HOME/aidash/zai.key`
- `CLAUDE_CRED_FILE`: `$HOME/.claude/.credentials.json`
- `CODEX_AUTH_FILE`: `$HOME/.codex/auth.json`

---

## Manage with `ctl.sh`

```bash
./ctl.sh start
./ctl.sh status
./ctl.sh logs
./ctl.sh stop
```

Log file: `aidash.log` (created while running via script)

Default script overrides:

- `AIDASH_INTERVAL` = `60`
- `AIDASH_LISTEN` = `0.0.0.0:8000`

Example with custom values:

```bash
AIDASH_INTERVAL=120 AIDASH_LISTEN=127.0.0.1:9000 ./ctl.sh start
```

---

## API examples

### `GET /data.json`

```json
{
  "collected": "2026-08-06T00:00:00Z",
  "ts": 1754448000,
  "any_error": false,
  "providers": [
    {
      "provider": "claude",
      "status": "ok",
      "plan": null,
      "limits": [
        {
          "label": "5-hour",
          "used_percent": 42.3,
          "remaining_percent": 57.7,
          "resets_at": 1754486400
        }
      ]
    }
  ]
}
```

### `GET /history.json`

Array of snapshots, each containing trimmed provider usage points for sparkline rendering.

---

## Required credentials

- z.ai: token text file (path from `ZAI_KEY_FILE`)
- Claude: JSON file with `claudeAiOauth.accessToken` under the `CLAUDE_CRED_FILE`
- Codex: JSON file with:
  - `tokens.access_token`
  - `tokens.refresh_token` (used when access token expires, unless `CODEX_DISABLE_REFRESH=1`)

Never commit these credential files.

---

## Notes

- Parsing depends on current provider API response shape; schema changes may require code updates.
- Keep credentials on trusted machines only.
- Frontend refreshes data every 60 seconds by default.

---

## License / scope

This is a personal dashboard project intended for local use.
