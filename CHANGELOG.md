# Changelog

All notable changes to this project will be documented in this file.

## [1.4.2] - 2026-11-13

### Added

- **Reset bank display** for z.ai, Codex and Claude: the number of window-reset credits each account still holds, shown as a `🔄 Reset bank` line at the bottom of the card with the soonest expiry date. z.ai reads `api.z.ai/api/biz/customer-package-reset/list?targetType=PERSONAL` (counts available `fiveHourResets`/`weekResets`); Codex reads `rate_limit_reset_credits.available_count` already present in the `wham/usage` payload, plus `wham/rate-limit-reset-credits` for title/expiry when any are held; Claude reads the `cedar_ember` grants from `/api/oauth/usage?cedar_ember=1&skip_spend=1` (asked with a `claude-cli/<ver>` User-Agent — with a `claude-code/<ver>` UA the endpoint answers `eligible:false, reason:"surface"`), grouped **Full** (a grant clearing the weekly window) vs model-named grants parsed from the label (**Fable**) with per-group expiry. All read-only passive endpoints; spending a reset stays on the vendor's own page (z.ai has no spend API, and aidash never spends Codex credits or Claude grants). Fields fail soft: a reset-endpoint error leaves the field absent instead of failing the provider.
- Replaced the outdated `ui-page.svg` mockup with a real screenshot (`ui-page.png`, headless-Chromium capture of the live dashboard).

## [1.4.1] - 2026-11-13

### Fixed

- **Codex window labels**: the main `rate_limit` windows now display as **5-hour** / **Weekly** (derived from the real `limit_window_seconds` value, 18000/604800) instead of the raw API field names `primary` / `secondary`, matching the existing `additional_rate_limits` labeling and the other providers. Unknown durations fall back to the raw `primary`/`secondary` name, so an OpenAI window reshuffle (e.g. the announced December 2026 ChatGPT+Codex allowance merge) cannot produce a wrong label.
- Dashboard load-error message pointed to `collect.sh`, which no longer exists in the project. It now suggests `./ctl.sh start` when `data.json` cannot be fetched.

### Verified (2026-11-13 API audit)

- All provider endpoints queried live and parsers confirmed against raw responses: z.ai, Claude (cswap), Codex, Antigravity, Muse.
- Antigravity verified field-by-field: `retrieveUserQuotaSummary` still returns only `5h`/`weekly` buckets, and `fetchAvailableModels` model list (19 models → 4 internal `chat_*`/`tab_*` filtered → tiered normalized) matches the dashboard pools exactly (Gemini ×12, Claude + GPT ×3). No update needed.
- OpenCode Go endpoint is alive and the key authenticates, but returns 403 `EntitlementError: "OpenCode Go subscription required."` — an account subscription state, not an API change. Resubscribe or provide a new key to restore the card.

## [1.4.0] - 2026-10-22

### Added

- **Claude view modes**: Direct remains the default and preserves one-account polling with token auto-refresh. A dashboard mode button switches collection to read-only multi-account usage from `claude-swap list --json`; aidash never reads or decrypts cswap credentials. Only selected mode is polled, avoiding duplicate usage requests.
- The mode button changes the dashboard data source only. Account switching remains a separate `claude-swap switch <number|email>` terminal command. `CLAUDE_SWAP_BIN` selects the executable.
- Multi-account freshness follows claude-swap's adaptive cache/poll schedule, which may be several minutes; aidash's collection interval does not force an upstream poll.
- `/history.json` snapshots include `account` so per-account sparklines stay separate.

### Changed

- Claude provider objects may include `account`, `source`, `active`, and `account_number`; dashboard shows the selected direct or claude-swap mode.

## [1.3.2] - 2026-10-18

### Fixed

- **Antigravity Gemini quota was stale** — dashboard showed the Gemini pool pinned at 100% remaining / 0% used while the `agy` CLI showed live usage. Root cause: the collector identified as a plain legacy client (`antigravity/1.0.0`) against the prod host (`cloudcode-pa.googleapis.com`), which serves a separate, lagging Gemini quota pool. The Claude + GPT (3p) buckets were identical on both hosts, which masked the bug.

### Changed

- **User-Agent**: `antigravity/hub/2.9.1 linux/amd64` — the server gates on the `antigravity/hub/` prefix and serves the current model list only for version ≥ 2.9 (refs: steipete/CodexBar#4102).
- **Host strategy**: requests now try `https://daily-cloudcode-pa.googleapis.com` (the live pool the `agy` CLI reads from; refs: decolua/9router#3891) first and fall back to prod on 403/404/5xx. `401` passes through so token refresh still works. `ANTIGRAVITY_BASE_URL` still overrides everything.
- Model list freshness: `fetchAvailableModels` on the daily + hub UA path returns newer entries (e.g. `gemini-3.8-flash-high` with `displayName`) that prod hid behind an unnamed `-tiered` alias.

### Verified

- Dashboard numbers now match `agy` `/usage` exactly (Gemini weekly 95.98% remaining, reset `2026-10-25T04:05:13Z` = "Refreshes in 166h 57m"; Gemini 5h 88.41% remaining, reset `09:05:13Z` = "3h 57m").

## [1.3.1] - 2026-10-16

### Fixed

- **Antigravity provider quota**: restored dual-window quota (`5-hour` and `Weekly`) for Gemini models by pointing default `AG_BASE` to production `https://cloudcode-pa.googleapis.com` (which returns both `Gemini Models` and `Claude and GPT models`). Added `ANTIGRAVITY_BASE_URL` environment variable override.
- **Antigravity quota fallback**: ensure models not represented in `retrieveUserQuotaSummary` are always collected from `fetchAvailableModels` `quotaInfo` rather than being omitted when 3rd-party models are returned.
- **Tiered model support**: clean up `-tiered` suffixes (e.g. `gemini-3.8-flash-tiered` → `gemini-3.8-flash`) so latest generation models are properly included in the Gemini quota pool and model list.

## [1.3.0] - 2026-10-04

### Added

- **Muse Code provider**: subscription quota via `POST https://api.meta.ai/muse-code/key` (same request the `muse` CLI makes at startup) with the OAuth device-code token from `providers.meta.access_token` in `~/.config/muse/auth.json` (override with `MUSE_AUTH_PATH`). Reports the **5-hour** window (`used_percent`, `window_duration_mins`, `resets_at`) and **Weekly** window from the `subs_usage` block, with plan badge from `subs_tier_name` (e.g. `Muse Code High Usage`). An active plan with no `subs_usage` block (idle window) shows an empty card, never an invented 0%. The minted `api_key` in the response is discarded and never logged. Dashboard card ♾️ muse (pink) with the standard bars, sparklines, and reset countdowns.

## [1.2.1] - 2026-09-25

### Fixed

- Antigravity pools: dedup deprecated model aliases using the API's `deprecatedModelIds` map (e.g. `gemini-3.1-pro-high` → `gemini-pro-agent`). Aliased models previously inflated the pool count (Gemini pool ×17 → ×16) while sharing one underlying model. Applied to both the primary path (`retrieveUserQuotaSummary` pool lists) and the fallback path (`fetchAvailableModels` remainingFraction grouping), with deprecation-chain resolution (up to 5 hops).

## [1.2.0] - 2026-09-14

### Added

- **OpenCode Go provider**: quota monitoring via `GET https://opencode.ai/zen/go/v1/usage` with Bearer API-key auth, covering all three usage windows: `5-hour` (rolling), `Weekly`, and `Monthly`, each with used percent and reset countdown.
- API key is read from a plain text file (default `$HOME/aidash/opencode-go.key`, perm 0600; override with `OPENCODE_GO_KEY_FILE`). The key is sent as `Authorization: Bearer <key>` — passive read-only usage endpoint, does not consume quota.
- Dashboard card 🏃 opencode-go (cyan) with the standard bars, sparklines, and reset countdowns.

### Docs

- README/AGENTES.md: OpenCode Go added to provider list, env defaults, required credentials, and `/data.json` example.

## [1.1.6] - 2026-09-04

### Fixed

- Antigravity API base URL: point `AG_BASE` to `https://daily-cloudcode-pa.googleapis.com` (matching the Antigravity CLI binary environment). Previously it connected to production `cloudcode-pa.googleapis.com` which tracked a separate/stale quota bucket for the same account.
- Antigravity quota collection: integrate `/v1internal:retrieveUserQuotaSummary` to report dual-window quotas (`(5-hour)` and `(Weekly)`) for both Gemini and 3rd-party (Claude + GPT) pools. Previously, `fetchAvailableModels` only exposed the weekly window for Gemini, causing the dashboard to show 100% exhaustion even when the 5-hour quota had reset.
- Fixed `pool_window` to determine cycle window from `resets_at` timestamp rather than hardcoded model family.

### Fixed

- Claude data collection: automatically refresh expired OAuth access tokens via Anthropic's platform token endpoint, persist rotated tokens to credentials file, and properly handle API error states on the dashboard instead of showing blank data.

## [1.1.4] - 2026-08-20

### Changed

- Codex per-model limits (`additional_rate_limits`, e.g. GPT-5.3-Codex-Spark) now report both windows: `(5-hour)` primary and `(Weekly)` secondary, with `window_seconds` included. Previously only the primary window was shown under a `model:<name>` label.

## [1.1.3] - 2026-08-17

### Changed

- Antigravity pool labels now explicitly state the quota cycle window: `(Weekly)` for 3rd-party models (Claude + GPT pool) and `(5-hour)` for Gemini models (e.g. `Claude + GPT (Weekly) pool ×3`, `Gemini (5-hour) pool ×16`).

## [1.1.2] - 2026-08-16

### Docs

- README: `ANTIGRAVITY_TOKEN_FILE` added to env defaults and required-credentials sections; `/data.json` example now includes an Antigravity provider (pool row + limit-hit row); corrected `ctl.sh` default interval (120, not 60)
- AGENTES.md: Antigravity added to provider list, environment variables, and data model notes (`models` field, `unknown` rows, limit-hit rows)

## [1.1.1] - 2026-08-16

### Fixed

- Antigravity pools that hit the 5-hour limit showed 0% used instead of 100%. The API reports an exhausted pool as `quotaInfo` present but `remainingFraction: null` (with `resetTime` still set); previously `null` defaulted to "nothing used". Exhausted pools now render as **limit hit** rows (100%, red) with reset countdown, distinct from models with no quota data at all.

## [1.1.0] - 2026-08-14

### Added

- **Antigravity provider** (Google's Antigravity IDE): per-model quota monitoring via the Gemini Cloud Code internal API (`cloudcode-pa.googleapis.com/v1internal:fetchAvailableModels`), using the local OAuth token from `~/.gemini/antigravity-cli/antigravity-oauth-token`.
- Models sharing an identical `remainingFraction` are collapsed into one row per quota pool (e.g. "Gemini pool ×16"); click a pool row to expand the member list.
- Models where the API omits `quotaInfo` are grouped into a separate grey "no quota data" row instead of being merged into a pool.
- Plan/tier resolution via `v1internal:loadCodeAssist` (e.g. `g1-pro-tier`).
- Token auto-refresh through `oauth2.googleapis.com` on 401 (read-only; the token file is never written back).
- Antigravity card in the dashboard UI with pool dropdown, tooltips, and 5-hour rolling-window reset countdown.

### Notes

- The Cloud Code API rejects requests without an `antigravity/<version>` User-Agent (403); the collector sends one.
- `resetTime` is reported inconsistently by the API (cached/omitted across calls); grouping keys on `remainingFraction` only and takes any reset time present in the group.

## [1.0.2] - 2026-08-13

### Fixed

- The 1.0.1 retry was insufficient: when the reqwest idle pool handed back a half-dead rustls/HTTP-2 connection, both the initial send and its single retry failed at the transport layer, leaving Codex with no data.
- Disabled keep-alive pooling entirely (`pool_max_idle_per_host(0)`) so every request opens a fresh connection — matching `curl` behavior, which was 100% reliable in testing. This removes the stale-connection failure mode at the root.
- Retries increased to 3 attempts (2 s then 4 s backoff) for tolerance of genuine short network blips.

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
