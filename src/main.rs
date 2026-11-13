use axum::{
    extract::State,
    http::{header, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use chrono::Utc;
use serde_json::{json, Value};
use std::env;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, RwLock};

const HTML: &str = include_str!("../index.html");
const CODEX_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const CLAUDE_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const ZAI_UA: &str = "aidash/0.1";
const CLAUDE_UA: &str = "claude-cli/2.1.323 (external, cli)";
// Antigravity talks to the Gemini Cloud Code internal API.
// Server rejects requests without an "antigravity/..." User-Agent (403).
// A plain "antigravity/<ver>" UA gets served a stale/legacy quota pool; the
// live pool (what the agy CLI /usage panel shows) requires the Hub client UA,
// version >= 2.9 (see steipete/CodexBar#4102).
const AG_UA: &str = "antigravity/hub/2.9.1 linux/amd64";
const AG_CLIENT_ID: &str = "1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com";
const AG_CLIENT_SECRET: &str = "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";
const AG_BASE: &str = "https://cloudcode-pa.googleapis.com";
// Quota summary / model list on the prod host can lag or pin at 100% remaining;
// the daily host serves the live numbers the agy CLI itself reads from
// (see decolua/9router#3891). ag_post tries this first, then falls back.
const AG_DAILY_BASE: &str = "https://daily-cloudcode-pa.googleapis.com";
const MUSE_KEY_URL: &str = "https://api.meta.ai/muse-code/key";

#[derive(Clone)]
struct ClaudeCred {
    label: Option<String>,
    path: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ClaudeMode {
    Direct,
    Multi,
}

impl ClaudeMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::Multi => "multi",
        }
    }
}

#[derive(Clone)]
struct Cfg {
    zai_key_file: PathBuf,
    opencode_go_key_file: PathBuf,
    claude_creds: Vec<ClaudeCred>,
    claude_swap_bin: Option<PathBuf>,
    codex_auth_file: PathBuf,
    antigravity_token_file: PathBuf,
    muse_auth_file: PathBuf,
    listen: String,
    interval: Duration,
}

#[derive(Clone)]
struct AppState {
    cfg: Cfg,
    data: Arc<RwLock<Value>>,
    history: Arc<RwLock<Vec<Value>>>,
    claude_mode: Arc<RwLock<ClaudeMode>>,
    collect_lock: Arc<Mutex<()>>,
    client: reqwest::Client,
}

// CLAUDE_CRED_FILES accepts comma-separated entries as `path` or
// `label=path`. Duplicate paths are ignored (they would race on refresh
// write-back and render duplicate cards).
fn parse_claude_creds(list: &str) -> Vec<ClaudeCred> {
    let mut seen = std::collections::HashSet::new();
    list.split(',')
        .filter_map(|entry| {
            let entry = entry.trim();
            if entry.is_empty() {
                return None;
            }
            let (label, path) = match entry.split_once('=') {
                Some((label, path)) if !label.trim().is_empty() && !path.trim().is_empty() => {
                    (Some(label.trim().to_string()), path.trim())
                }
                _ => (None, entry),
            };
            let cred = ClaudeCred {
                label,
                path: PathBuf::from(path),
            };
            seen.insert(cred.path.clone()).then_some(cred)
        })
        .collect()
}

fn claude_creds_from_env(home: &str) -> (Vec<ClaudeCred>, Option<PathBuf>) {
    let swap_bin = find_claude_swap(home);
    if let Ok(list) = env::var("CLAUDE_CRED_FILES") {
        let creds = parse_claude_creds(&list);
        if !creds.is_empty() {
            return (creds, swap_bin);
        }
    }
    if let Ok(path) = env::var("CLAUDE_CRED_FILE") {
        if !path.trim().is_empty() {
            return (
                vec![ClaudeCred {
                    label: None,
                    path: PathBuf::from(path.trim()),
                }],
                swap_bin,
            );
        }
    }
    let path = PathBuf::from(format!("{home}/.claude/.credentials.json"));
    (
        vec![ClaudeCred { label: None, path }],
        swap_bin,
    )
}

fn find_claude_swap(home: &str) -> Option<PathBuf> {
    if let Ok(bin) = env::var("CLAUDE_SWAP_BIN") {
        let bin = bin.trim();
        // Explicitly empty disables claude-swap discovery.
        return (!bin.is_empty()).then(|| PathBuf::from(bin));
    }
    let local = PathBuf::from(format!("{home}/.local/bin/claude-swap"));
    if local.is_file() {
        return Some(local);
    }
    if let Some(paths) = env::var_os("PATH") {
        for dir in env::split_paths(&paths) {
            let candidate = dir.join("claude-swap");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

impl Cfg {
    fn from_env() -> Self {
        let home = env::var("HOME").unwrap_or_else(|_| "/root".into());
        let (claude_creds, claude_swap_bin) = claude_creds_from_env(&home);
        Self {
            zai_key_file: PathBuf::from(env::var("ZAI_KEY_FILE").unwrap_or_else(|_| format!("{home}/aidash/zai.key"))),
            opencode_go_key_file: PathBuf::from(
                env::var("OPENCODE_GO_KEY_FILE").unwrap_or_else(|_| format!("{home}/aidash/opencode-go.key")),
            ),
            claude_creds,
            claude_swap_bin,
            codex_auth_file: PathBuf::from(
                env::var("CODEX_AUTH_FILE").unwrap_or_else(|_| format!("{home}/.codex/auth.json")),
            ),
            antigravity_token_file: PathBuf::from(
                env::var("ANTIGRAVITY_TOKEN_FILE")
                    .unwrap_or_else(|_| format!("{home}/.gemini/antigravity-cli/antigravity-oauth-token")),
            ),
            muse_auth_file: {
                if let Ok(p) = env::var("MUSE_AUTH_PATH") {
                    PathBuf::from(p)
                } else if let Ok(xdg) = env::var("XDG_CONFIG_HOME") {
                    if !xdg.is_empty() {
                        PathBuf::from(format!("{xdg}/muse/auth.json"))
                    } else {
                        PathBuf::from(format!("{home}/.config/muse/auth.json"))
                    }
                } else {
                    PathBuf::from(format!("{home}/.config/muse/auth.json"))
                }
            },
            listen: env::var("AIDASH_LISTEN").unwrap_or_else(|_| "0.0.0.0:8000".into()),
            interval: Duration::from_secs(
                env::var("AIDASH_INTERVAL").ok().and_then(|s| s.parse().ok()).unwrap_or(300),
            ),
        }
    }
}

fn read_file(p: &PathBuf) -> Result<String, String> {
    std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))
}

fn is_ok_status(v: &Value) -> bool {
    v.get("status").and_then(|s| s.as_str()) == Some("ok")
}

// ---------- z.ai ----------
async fn collect_zai(cfg: &Cfg, client: &reqwest::Client) -> Value {
    let key = match read_file(&cfg.zai_key_file) {
        Ok(k) => k.trim().to_string(),
        Err(e) => return json!({"provider":"z.ai","status":"error","error":e}),
    };
    let resp = client
        .get("https://api.z.ai/api/monitor/usage/quota/limit")
        .header(header::AUTHORIZATION, &key)
        .header(header::USER_AGENT, ZAI_UA)
        .send()
        .await;
    let body = match resp {
        Ok(r) => r.text().await.unwrap_or_default(),
        Err(e) => return json!({"provider":"z.ai","status":"error","error":e.to_string()}),
    };
    let v: Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(_) => return json!({"provider":"z.ai","status":"error","error":"bad json"}),
    };
    let limits = v.pointer("/data/limits").and_then(|l| l.as_array()).cloned().unwrap_or_default();
    let level = v.pointer("/data/level").and_then(|l| l.as_str()).unwrap_or("").to_uppercase();
    let out: Vec<Value> = limits
        .iter()
        .filter_map(|l| {
            let unit = l.get("unit").and_then(|u| u.as_i64()).unwrap_or(0);
            let label = match unit {
                3 => "5-hour",
                5 => "MCP/month",
                6 => "Weekly",
                _ => return None,
            };
            let pct = l.get("percentage").and_then(|p| p.as_f64()).unwrap_or(0.0);
            Some(json!({
                "label": label,
                "used_percent": pct,
                "remaining_percent": (100.0 - pct).max(0.0),
                "current": l.get("currentValue"),
                "usage": l.get("usage"),
                "remaining": l.get("remaining"),
                "resets_at": l.get("nextResetTime").and_then(|t| t.as_f64()).map(|t| t / 1000.0),
            }))
        })
        .collect();
    // Reset bank: window resets the plan may spend on z.ai's own usage
    // page (read-only; no API to spend one). Best-effort: a failure just
    // leaves the field out.
    let resets_resp = client
        .get("https://api.z.ai/api/biz/customer-package-reset/list?targetType=PERSONAL")
        .header(header::AUTHORIZATION, &key)
        .header(header::USER_AGENT, ZAI_UA)
        .send()
        .await;
    let resets = match resets_resp {
        Ok(r) => r
            .text()
            .await
            .ok()
            .and_then(|body| serde_json::from_str::<Value>(&body).ok())
            .and_then(|v| zai_reset_bank(v.pointer("/data"))),
        Err(_) => None,
    }
    .unwrap_or(Value::Null);
    json!({"provider":"z.ai","status":"ok","plan":level,"limits":out,"resets":resets})
}

// Counts of the still-available window resets the plan holds and when
// the soonest one runs out (the vendor's "YYYY-MM-DD HH:MM:SS" string;
// the frontend shows its date part).
fn zai_reset_bank(data: Option<&Value>) -> Option<Value> {
    let data = data?;
    let count = |arr: &str| -> (i64, Option<String>) {
        let mut n = 0;
        let mut soonest: Option<String> = None;
        if let Some(list) = data.get(arr).and_then(|l| l.as_array()) {
            for r in list {
                if r.get("available").and_then(|a| a.as_bool()) != Some(true) {
                    continue;
                }
                n += 1;
                if let Some(e) = r.get("expireTime").and_then(|t| t.as_str()) {
                    if soonest.as_deref().map_or(true, |s| e < s) {
                        soonest = Some(e.to_string());
                    }
                }
            }
        }
        (n, soonest)
    };
    let (five_hour, fh_exp) = count("fiveHourResets");
    let (weekly, w_exp) = count("weekResets");
    if five_hour == 0 && weekly == 0 {
        return None;
    }
    let expires_at = [fh_exp, w_exp].into_iter().flatten().min();
    Some(json!({
        "five_hour": five_hour,
        "weekly": weekly,
        "expires_at": expires_at,
    }))
}

// ---------- OpenCode Go ----------
async fn collect_opencode_go(cfg: &Cfg, client: &reqwest::Client) -> Value {
    let err = |e: String| json!({"provider":"opencode-go","status":"error","error":e});
    let key = match read_file(&cfg.opencode_go_key_file) {
        Ok(k) => k.trim().to_string(),
        Err(e) => return err(e),
    };
    if key.is_empty() {
        return err("key file empty".into());
    }
    let resp = client
        .get("https://opencode.ai/zen/go/v1/usage")
        .header(header::AUTHORIZATION, format!("Bearer {key}"))
        .header(header::ACCEPT, "application/json")
        .header(header::USER_AGENT, ZAI_UA)
        .send()
        .await;
    let resp = match resp {
        Ok(r) => r,
        Err(e) => return err(e.to_string()),
    };
    let status = resp.status().as_u16();
    let body = match resp.text().await {
        Ok(b) => b,
        Err(e) => return err(format!("read body: {e}")),
    };
    if status != 200 {
        let snippet: String = body.chars().take(160).collect();
        return err(format!("http {status}: {snippet}"));
    }
    let v: Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(_) => return err("bad json".into()),
    };
    let mut limits = Vec::new();
    for (key, label) in [("rolling", "5-hour"), ("weekly", "Weekly"), ("monthly", "Monthly")] {
        let w = match v.pointer(&format!("/usage/{key}")) {
            Some(w) if !w.is_null() => w,
            _ => continue,
        };
        let pct = w.get("percent").and_then(|p| p.as_f64()).unwrap_or(0.0);
        let resets_at = w
            .get("resetsAt")
            .and_then(|t| t.as_str())
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.timestamp() as f64);
        limits.push(json!({
            "label": label,
            "used_percent": pct,
            "remaining_percent": (100.0 - pct).max(0.0),
            "resets_at": resets_at,
        }));
    }
    if limits.is_empty() {
        return err("no usage windows found in response".into());
    }
    json!({"provider":"opencode-go","status":"ok","plan":"Go","limits":limits})
}

// ---------- Claude ----------
fn claude_ts(v: &Value) -> Option<f64> {
    let s = v.as_str()?;
    let s = s.trim_end_matches('Z');
    chrono::DateTime::parse_from_rfc3339(s).ok().map(|dt| dt.timestamp() as f64)
}

fn claude_cred_email(raw: &Value) -> Option<String> {
    ["/claudeAiOauth/emailAddress", "/claudeAiOauth/email"]
        .iter()
        .find_map(|ptr| raw.pointer(ptr).and_then(|e| e.as_str()))
        .filter(|e| !e.is_empty())
        .map(str::to_string)
}

// Claude Code keeps the account email in `<config-dir>/.claude.json`
// (`oauthAccount.emailAddress`). The default layout is the exception:
// `~/.claude/.credentials.json` pairs with `~/.claude.json`.
fn claude_config_email(cred_path: &Path) -> Option<String> {
    let file = cred_path.file_name()?.to_str()?;
    if file != ".credentials.json" && file != "credentials.json" {
        return None;
    }
    let dir = cred_path.parent()?;
    let mut candidates = vec![dir.join(".claude.json")];
    if dir.file_name().and_then(|s| s.to_str()) == Some(".claude") {
        if let Some(home) = dir.parent() {
            candidates.push(home.join(".claude.json"));
        }
    }
    candidates.iter().find_map(|path| {
        let raw = std::fs::read_to_string(path).ok()?;
        let v: Value = serde_json::from_str(&raw).ok()?;
        v.pointer("/oauthAccount/emailAddress")
            .and_then(|e| e.as_str())
            .filter(|e| !e.is_empty())
            .map(str::to_string)
    })
}

fn claude_path_label(path: &Path) -> String {
    let file = path.file_name().and_then(|s| s.to_str()).unwrap_or("account");
    if file == ".credentials.json" || file == "credentials.json" {
        path.parent()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            .map(|s| s.trim_start_matches('.'))
            .filter(|s| !s.is_empty() && *s != "claude")
            .unwrap_or("account")
            .to_string()
    } else {
        file.strip_suffix(".json").unwrap_or(file).to_string()
    }
}

async fn claude_get(client: &reqwest::Client, token: &str) -> Result<(u16, String), String> {
    let resp = client
        .get("https://api.anthropic.com/api/oauth/usage?cedar_ember=1&skip_spend=1")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::USER_AGENT, CLAUDE_UA)
        .send()
        .await
        .map_err(|e| format!("send: {e}"))?;
    let status = resp.status().as_u16();
    let body = resp.text().await.map_err(|e| format!("read body: {e}"))?;
    Ok((status, body))
}

async fn claude_refresh(client: &reqwest::Client, refresh: &str) -> Result<(String, String, Option<f64>, Option<f64>), String> {
    let r = client
        .post("https://platform.claude.com/v1/oauth/token")
        .header(header::CONTENT_TYPE, "application/json")
        .json(&json!({
            "grant_type": "refresh_token",
            "refresh_token": refresh,
            "client_id": CLAUDE_CLIENT_ID,
        }))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let body = r.text().await.unwrap_or_default();
    let v: Value = serde_json::from_str(&body).map_err(|e| format!("refresh parse: {e}"))?;
    if let Some(err) = v.get("error") {
        let desc = v.get("error_description").and_then(|d| d.as_str()).unwrap_or("");
        return Err(format!("{err}: {desc}"));
    }
    let at = v.get("access_token").and_then(|t| t.as_str()).ok_or_else(|| format!("refresh failed: {body}"))?.to_string();
    let rt = v.get("refresh_token").and_then(|t| t.as_str()).unwrap_or(refresh).to_string();
    let now_ms = Utc::now().timestamp_millis() as f64;
    let expires_at = v.get("expires_in").and_then(|e| e.as_f64()).map(|s| now_ms + s * 1000.0);
    let rt_expires_at = v.get("refresh_token_expires_in").and_then(|e| e.as_f64()).map(|s| now_ms + s * 1000.0);
    Ok((at, rt, expires_at, rt_expires_at))
}

fn save_claude_creds(
    path: &Path,
    mut raw_json: Value,
    new_at: &str,
    new_rt: &str,
    expires_at: Option<f64>,
    rt_expires_at: Option<f64>,
) -> Result<(), String> {
    if let Some(oauth) = raw_json.get_mut("claudeAiOauth").and_then(|o| o.as_object_mut()) {
        oauth.insert("accessToken".to_string(), json!(new_at));
        oauth.insert("refreshToken".to_string(), json!(new_rt));
        if let Some(exp) = expires_at {
            oauth.insert("expiresAt".to_string(), json!(exp as i64));
        }
        if let Some(rt_exp) = rt_expires_at {
            oauth.insert("refreshTokenExpiresAt".to_string(), json!(rt_exp as i64));
        }
    }
    let s = serde_json::to_string_pretty(&raw_json).map_err(|e| e.to_string())?;
    std::fs::write(path, s).map_err(|e| format!("{}: {e}", path.display()))
}

async fn collect_claude_account(
    cred: &ClaudeCred,
    client: &reqwest::Client,
) -> (Option<String>, String, Value) {
    let fallback = claude_path_label(&cred.path);
    let base_label = cred.label.clone().or_else(|| claude_config_email(&cred.path));
    match claude_account_usage(cred, client).await {
        Ok((email, data)) => (cred.label.clone().or(email).or(base_label), fallback, data),
        Err(e) => (
            base_label,
            fallback,
            json!({"provider":"claude","status":"error","error":e}),
        ),
    }
}

async fn claude_account_usage(
    cred: &ClaudeCred,
    client: &reqwest::Client,
) -> Result<(Option<String>, Value), String> {
    let raw = read_file(&cred.path)?;
    let parsed_cred = serde_json::from_str::<Value>(&raw).map_err(|e| format!("parse creds: {e}"))?;
    let email = claude_cred_email(&parsed_cred);
    let mut token = parsed_cred
        .pointer("/claudeAiOauth/accessToken")
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .to_string();
    let refresh = parsed_cred
        .pointer("/claudeAiOauth/refreshToken")
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .to_string();
    let plan = parsed_cred
        .pointer("/claudeAiOauth/subscriptionType")
        .and_then(|s| s.as_str())
        .map(|s| s.to_uppercase());

    if token.is_empty() && refresh.is_empty() {
        return Err("no access or refresh token found (run `claude` to login)".into());
    }

    let (mut status, mut body) = if !token.is_empty() {
        claude_get(client, &token).await?
    } else {
        (401, String::new())
    };

    let mut parsed: Result<Value, _> = serde_json::from_str(&body);
    let needs_refresh = status == 401 || status == 403 || status == 429
        || parsed.as_ref().ok().and_then(|v| v.get("error")).is_some();

    if needs_refresh && !refresh.is_empty() {
        match claude_refresh(client, &refresh).await {
            Ok((at, rt, exp, rt_exp)) => {
                token = at.clone();
                let _ = save_claude_creds(&cred.path, parsed_cred.clone(), &at, &rt, exp, rt_exp);
                match claude_get(client, &token).await {
                    Ok((s, b)) => {
                        status = s;
                        body = b;
                        parsed = serde_json::from_str(&body);
                    }
                    Err(e) => return Err(e),
                }
            }
            Err(e) => {
                if status != 200 {
                    return Err(format!("refresh token failed ({e}) — please re-login via `claude`"));
                }
            }
        }
    }

    if status != 200 {
        let snippet: String = body.chars().take(160).collect();
        return Err(format!("http {status}: {snippet}"));
    }

    let v: Value = match parsed {
        Ok(v) => v,
        Err(e) => {
            let snippet: String = body.chars().take(160).collect();
            return Err(format!("parse: {e}; body: {snippet}"));
        }
    };

    if let Some(error_val) = v.get("error") {
        return Err(format!("{error_val}"));
    }

    let mut limits = Vec::new();
    for (key, label) in [("five_hour", "5-hour"), ("seven_day", "7-day")] {
        if let Some(w) = v.get(key) {
            let pct = w.get("utilization").and_then(|p| p.as_f64()).unwrap_or(0.0);
            limits.push(json!({
                "label": label,
                "used_percent": pct,
                "remaining_percent": (100.0 - pct).max(0.0),
                "resets_at": w.get("resets_at").and_then(claude_ts),
            }));
        }
    }

    if limits.is_empty() {
        return Err("no usage limits found in Claude API response".into());
    }

    let resets = claude_reset_bank(&v);
    Ok((email, json!({"provider":"claude","status":"ok","plan":plan,"limits":limits,"resets":resets})))
}

// Reset bank (cedar_ember): usage-limit reset grants the subscription
// still holds — the same grants Claude Code's /limit-reset spends
// (POST /api/organizations/{org}/reset_rate_limits). Display only;
// aidash never spends one. The usage endpoint reports them only when
// asked as Claude Code asks (cedar_ember=1, a claude-cli/<ver> User-Agent
// — a claude-code/<ver> UA is told "ineligible: surface").
fn claude_reset_bank(v: &Value) -> Value {
    let ce = match v.get("cedar_ember") {
        Some(c) if !c.is_null() => c,
        _ => return Value::Null,
    };
    if ce.get("eligible").and_then(|e| e.as_bool()) != Some(true) {
        return Value::Null;
    }
    let now = Utc::now().timestamp();
    let held: Vec<&Value> = ce
        .get("grants")
        .and_then(|g| g.as_array())
        .map(|a| {
            a.iter()
                .filter(|g| {
                    let left = g.get("resets_left").and_then(|r| r.as_i64()).unwrap_or(0);
                    let paused = g.get("paused").and_then(|p| p.as_bool()) == Some(true);
                    let ended = g.get("ends_at").and_then(claude_ts).map(|t| t <= now as f64).unwrap_or(false);
                    left > 0 && !paused && !ended
                })
                .collect()
        })
        .unwrap_or_default();
    if held.is_empty() {
        return Value::Null;
    }
    // A grant that clears the weekly window is a full reset; the rest are
    // named by their label ("one Fable reset" -> "Fable"), shown as
    // separate groups: 1x Full + 2x Fable.
    let mut groups: Vec<(String, i64, Option<String>)> = Vec::new();
    for g in &held {
        let clears = g.get("clears").and_then(|c| c.as_array()).cloned().unwrap_or_default();
        let name = if clears.iter().any(|c| c.as_str() == Some("seven_day")) {
            "Full".to_string()
        } else {
            g.get("label").and_then(|l| l.as_str()).map_or_else(|| "Partial".to_string(), claude_grant_name)
        };
        let left = g.get("resets_left").and_then(|r| r.as_i64()).unwrap_or(0);
        let ends = g.get("ends_at").and_then(|e| e.as_str()).map(str::to_string);
        match groups.iter_mut().find(|(n, _, _)| *n == name) {
            Some((_, c, e)) => {
                *c += left;
                if ends.as_deref().map_or(false, |d| e.as_deref().map_or(true, |x| d < x)) {
                    *e = ends;
                }
            }
            None => groups.push((name, left, ends)),
        }
    }
    groups.sort_by(|a, b| {
        let full = (b.0 == "Full").cmp(&(a.0 == "Full"));
        full.then_with(|| a.0.cmp(&b.0))
    });
    let count: i64 = groups.iter().map(|(_, c, _)| c).sum();
    let groups_json: Vec<Value> = groups
        .iter()
        .map(|(name, c, ends)| json!({"label": name, "count": c, "expires_at": ends}))
        .collect();
    json!({"count": count, "groups": groups_json})
}

// "New model launch: one Fable reset ..." / "... one more Fable
// reset ..." -> "Fable"; a label naming no reset kind falls back to
// "Partial".
fn claude_grant_name(label: &str) -> String {
    if let Some(rel) = label.find(" reset") {
        let head = &label[..rel];
        if let Some(word) = head.rsplit(' ').next() {
            let generic = word.eq_ignore_ascii_case("one") || word.eq_ignore_ascii_case("more");
            if !word.is_empty()
                && !generic
                && word.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_')
            {
                return word.to_string();
            }
        }
    }
    "Partial".to_string()
}

// ---------- Claude (claude-swap) ----------
// Usage-only integration: claude-swap owns the encrypted credentials and its
// own polling cadence, so aidash never reads tokens and never refreshes them.
// The JSON schema is documented as additive in the claude-swap README.
async fn collect_claude_swap(bin: &Path) -> Result<Vec<Value>, String> {
    let output = tokio::time::timeout(
        Duration::from_secs(60),
        tokio::process::Command::new(bin)
            .args(["list", "--json"])
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| "claude-swap list timed out after 60s".to_string())?
    .map_err(|e| format!("spawn claude-swap: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let snippet: String = stderr.trim().chars().take(200).collect();
        return Err(format!("claude-swap list failed: {snippet}"));
    }
    let v: Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("parse claude-swap json: {e}"))?;
    claude_swap_providers(&v)
}

fn claude_swap_providers(v: &Value) -> Result<Vec<Value>, String> {
    let accounts = v
        .get("accounts")
        .and_then(|a| a.as_array())
        .ok_or_else(|| "claude-swap json has no accounts array".to_string())?;
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::with_capacity(accounts.len());
    for a in accounts {
        let number = a.get("number").and_then(|n| n.as_i64()).unwrap_or(0);
        let email = a.get("email").and_then(|e| e.as_str()).unwrap_or("").trim();
        let base = if email.is_empty() { "account" } else { email };
        let mut account = base.to_string();
        if !seen.insert(account.clone()) {
            account = format!("{base} #{number}");
            seen.insert(account.clone());
        }
        let active = a.get("active").and_then(|b| b.as_bool()).unwrap_or(false);
        let usage_status = a
            .get("usageStatus")
            .and_then(|s| s.as_str())
            .unwrap_or("unavailable");

        let mut row = json!({
            "provider": "claude",
            "account": account,
            "source": "cswap",
            "email": email,
            "account_number": number,
        });
        if active {
            row["active"] = json!(true);
        }

        if usage_status != "ok" {
            row["status"] = json!("error");
            row["error"] = json!(claude_swap_error(a, usage_status));
            out.push(row);
            continue;
        }

        let usage = match a.get("usage").filter(|u| u.is_object()) {
            Some(u) => u,
            None => {
                row["status"] = json!("error");
                row["error"] = json!("usage ok but no usage windows returned");
                out.push(row);
                continue;
            }
        };
        let mut limits = Vec::new();
        for (key, label) in [("fiveHour", "5-hour"), ("sevenDay", "7-day")] {
            if let Some(w) = usage.get(key).filter(|w| w.is_object()) {
                let pct = w.get("pct").and_then(|p| p.as_f64()).unwrap_or(0.0);
                limits.push(json!({
                    "label": label,
                    "used_percent": pct,
                    "remaining_percent": (100.0 - pct).max(0.0),
                    "resets_at": w.get("resetsAt").and_then(claude_ts),
                }));
            }
        }
        if let Some(scoped) = usage.get("scoped").and_then(|s| s.as_array()) {
            for w in scoped {
                let name = w.get("name").and_then(|n| n.as_str()).unwrap_or("model");
                let pct = w.get("pct").and_then(|p| p.as_f64()).unwrap_or(0.0);
                limits.push(json!({
                    "label": format!("{name} (7-day)"),
                    "used_percent": pct,
                    "remaining_percent": (100.0 - pct).max(0.0),
                    "resets_at": w.get("resetsAt").and_then(claude_ts),
                }));
            }
        }
        if limits.is_empty() {
            row["status"] = json!("error");
            row["error"] = json!("no usage limits in claude-swap response");
        } else {
            row["status"] = json!("ok");
            row["limits"] = json!(limits);
        }
        out.push(row);
    }
    Ok(out)
}

fn claude_swap_error(a: &Value, status: &str) -> String {
    match status {
        "unavailable" => a
            .get("usageError")
            .and_then(|e| e.as_str())
            .map(|e| format!("usage unavailable ({e})"))
            .unwrap_or_else(|| "usage unavailable".into()),
        "token_expired" => "token expired (cswap will retry)".into(),
        "api_key" => "API key account — no subscription quota".into(),
        "keychain_unavailable" => "macOS Keychain unavailable".into(),
        "relogin_required" => "re-login needed (`cswap add`)".into(),
        "foreign_credential" => "credential belongs to another account".into(),
        "no_credentials" => "no credentials stored".into(),
        other => format!("usage status: {other}"),
    }
}

async fn collect_claude(cfg: &Cfg, client: &reqwest::Client, mode: ClaudeMode) -> Vec<Value> {
    match mode {
        ClaudeMode::Direct => collect_claude_files(cfg, client).await,
        ClaudeMode::Multi => {
            let Some(bin) = &cfg.claude_swap_bin else {
                return vec![json!({
                    "provider":"claude",
                    "status":"error",
                    "error":"claude-swap not found; set CLAUDE_SWAP_BIN"
                })];
            };
            match collect_claude_swap(bin).await {
                Ok(accounts) if !accounts.is_empty() => accounts,
                Ok(_) => vec![json!({
                    "provider":"claude",
                    "status":"error",
                    "error":"claude-swap has no managed accounts"
                })],
                Err(e) => vec![json!({
                    "provider":"claude",
                    "status":"error",
                    "error":e
                })],
            }
        }
    }
}

async fn collect_claude_files(cfg: &Cfg, client: &reqwest::Client) -> Vec<Value> {
    let multiple = cfg.claude_creds.len() > 1;
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::with_capacity(cfg.claude_creds.len());
    for (i, cred) in cfg.claude_creds.iter().enumerate() {
        let (preferred, fallback, mut value) = collect_claude_account(cred, client).await;
        let label = preferred.or_else(|| multiple.then_some(fallback));
        if let Some(mut label) = label {
            if !seen.insert(label.clone()) {
                label = format!("{label} #{}", i + 1);
                seen.insert(label.clone());
            }
            if let Some(obj) = value.as_object_mut() {
                obj.insert("account".into(), json!(label));
                obj.insert("source".into(), json!("direct"));
            }
        }
        out.push(value);
    }
    out
}

// ---------- Codex ----------
async fn codex_get(client: &reqwest::Client, url: &str, token: &str, account_id: &str) -> Result<(u16, String), String> {
    let do_req = || async {
        let mut req = client
            .get(url)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::ACCEPT, "application/json")
            .header(header::ORIGIN, "https://chatgpt.com");
        if !account_id.is_empty() {
            req = req.header("ChatGPT-Account-Id", account_id);
        }
        let r = req.send().await.map_err(|e| format!("send: {e}"))?;
        let status = r.status().as_u16();
        let body = r.text().await.map_err(|e| format!("read body: {e}"))?;
        Ok::<_, String>((status, body))
    };
    // retry on transient transport error (stale pooled connection, brief reset)
    let mut last_err = String::new();
    for attempt in 0..3u32 {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_secs(2u64 << (attempt - 1))).await; // 2s, then 4s
        }
        match do_req().await {
            Ok(x) => return Ok(x),
            Err(e) => last_err = format!("attempt {}: {e}", attempt + 1),
        }
    }
    Err(format!("all 3 attempts failed; last: {last_err}"))
}

async fn codex_refresh(client: &reqwest::Client, refresh: &str) -> Result<(String, Option<String>, Option<String>), String> {
    let r = client
        .post("https://auth.openai.com/oauth/token")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(format!(
            "grant_type=refresh_token&client_id={CODEX_CLIENT_ID}&refresh_token={}",
            urlenc(refresh)
        ))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let body = r.text().await.unwrap_or_default();
    let v: Value = serde_json::from_str(&body).map_err(|e| format!("refresh parse: {e}"))?;
    let at = v.get("access_token").and_then(|t| t.as_str()).ok_or("refresh failed")?.to_string();
    let rt = v.get("refresh_token").and_then(|t| t.as_str()).map(|s| s.to_string());
    let it = v.get("id_token").and_then(|t| t.as_str()).map(|s| s.to_string());
    Ok((at, rt, it))
}

fn urlenc(s: &str) -> String {
    // minimal percent-encoding for form body
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

async fn collect_codex(cfg: &Cfg, client: &reqwest::Client) -> Value {
    let auth = match read_file(&cfg.codex_auth_file) {
        Ok(a) => a,
        Err(e) => return json!({"provider":"codex","status":"error","error":e}),
    };
    let v: Value = match serde_json::from_str(&auth) {
        Ok(v) => v,
        Err(e) => return json!({"provider":"codex","status":"error","error":format!("parse auth: {e}")}),
    };
    let mut token = v
        .pointer("/tokens/access_token")
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .to_string();
    let account_id = v
        .pointer("/tokens/account_id")
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .to_string();
    if token.is_empty() {
        return json!({"provider":"codex","status":"error","error":"no access token"});
    }

    let usage_url = "https://chatgpt.com/backend-api/wham/usage";
    let (mut status, mut body) = match codex_get(client, usage_url, &token, &account_id).await {
        Ok(x) => x,
        Err(e) => return json!({"provider":"codex","status":"error","error":e}),
    };
    let mut parsed: Result<Value, _> = serde_json::from_str(&body);

    // 401/403 or error field -> try refresh (read-only; never write back to auth file)
    let needs_refresh = status == 401 || status == 403
        || parsed.as_ref().ok().and_then(|v| v.get("error")).is_some();
    if needs_refresh {
        if std::env::var("CODEX_DISABLE_REFRESH").ok().as_deref() == Some("1") {
            return json!({"provider":"codex","status":"error","error":format!("http {status}: token invalid"),"limits":[]});
        }
        let refresh = v
            .pointer("/tokens/refresh_token")
            .and_then(|t| t.as_str())
            .unwrap_or("");
        if !refresh.is_empty() {
            match codex_refresh(client, refresh).await {
                Ok((at, _, _)) => {
                    token = at;
                    match codex_get(client, usage_url, &token, &account_id).await {
                        Ok((s, b)) => {
                            status = s;
                            body = b;
                            parsed = serde_json::from_str(&body);
                        }
                        Err(e) => return json!({"provider":"codex","status":"error","error":e}),
                    }
                }
                Err(e) => return json!({"provider":"codex","status":"error","error":e}),
            }
        }
    }

    if status != 200 {
        let snippet: String = body.chars().take(160).collect();
        return json!({"provider":"codex","status":"error","error":format!("http {status}: {snippet}")});
    }
    let v: Value = match parsed {
        Ok(v) => v,
        Err(e) => {
            let snippet: String = body.chars().take(160).collect();
            return json!({"provider":"codex","status":"error","error":format!("parse: {e}; body: {snippet}")});
        }
    };
    if let Some(err) = v.get("error") {
        return json!({"provider":"codex","status":"error","error":err});
    }
    let plan = v.get("plan_type").and_then(|p| p.as_str()).unwrap_or("");
    let email = v.get("email").and_then(|e| e.as_str()).unwrap_or("");
    let rl = v.get("rate_limit").cloned().unwrap_or(Value::Null);
    let mut limits = Vec::new();
    if let Some(pw) = rl.get("primary_window") {
        let pct = pw.get("used_percent").and_then(|p| p.as_f64()).unwrap_or(0.0);
        let secs = pw.get("limit_window_seconds").and_then(|s| s.as_i64());
        let label = match secs {
            Some(18000) => "5-hour",
            Some(604800) => "Weekly",
            _ => "primary",
        };
        limits.push(json!({
            "label": label,
            "used_percent": pct,
            "remaining_percent": (100.0 - pct).max(0.0),
            "window_seconds": pw.get("limit_window_seconds"),
            "resets_at": pw.get("reset_at"),
        }));
    }
    if let Some(sw) = rl.get("secondary_window").filter(|s| !s.is_null()) {
        let pct = sw.get("used_percent").and_then(|p| p.as_f64()).unwrap_or(0.0);
        let secs = sw.get("limit_window_seconds").and_then(|s| s.as_i64());
        let label = match secs {
            Some(18000) => "5-hour",
            Some(604800) => "Weekly",
            _ => "secondary",
        };
        limits.push(json!({
            "label": label,
            "used_percent": pct,
            "remaining_percent": (100.0 - pct).max(0.0),
            "window_seconds": sw.get("limit_window_seconds"),
            "resets_at": sw.get("reset_at"),
        }));
    }
    if let Some(extra) = v.get("additional_rate_limits").and_then(|a| a.as_array()) {
        for m in extra {
            let name = m.get("limit_name").and_then(|n| n.as_str()).unwrap_or("model");
            let rl = m.get("rate_limit").cloned().unwrap_or(Value::Null);
            for (key, fallback) in [("primary_window", "primary"), ("secondary_window", "secondary")] {
                if let Some(pw) = rl.get(key).filter(|s| !s.is_null()) {
                    let pct = pw.get("used_percent").and_then(|p| p.as_f64()).unwrap_or(0.0);
                    let secs = pw.get("limit_window_seconds").and_then(|s| s.as_i64());
                    let label = match secs {
                        Some(18000) => format!("{name} (5-hour)"),
                        Some(604800) => format!("{name} (Weekly)"),
                        _ => format!("{name} ({fallback})"),
                    };
                    limits.push(json!({
                        "label": label,
                        "used_percent": pct,
                        "remaining_percent": (100.0 - pct).max(0.0),
                        "window_seconds": pw.get("limit_window_seconds"),
                        "resets_at": pw.get("reset_at"),
                    }));
                }
            }
        }
    }
    // Reset bank: rate-limit reset credits the account holds (display
    // only — a credit is spent from Codex's own UI). The count rides on
    // the usage payload already read; the credits' own passive endpoint
    // says when the soonest one runs out and what it is titled.
    let resets = codex_reset_bank(client, &token, &account_id, &v).await;
    json!({"provider":"codex","status":"ok","plan":plan,"email":email,"limits":limits,"resets":resets})
}

// Count of held reset credits from the usage payload, then best-effort
// detail read from wham/rate-limit-reset-credits: soonest expires_at and
// the first held credit's title.
async fn codex_reset_bank(client: &reqwest::Client, token: &str, account_id: &str, usage: &Value) -> Value {
    let count = usage
        .pointer("/rate_limit_reset_credits/available_count")
        .and_then(|c| c.as_i64())
        .unwrap_or(0);
    if count <= 0 {
        return Value::Null;
    }
    let mut out = json!({"count": count});
    if let Ok((status, body)) = codex_get(client, "https://chatgpt.com/backend-api/wham/rate-limit-reset-credits", token, account_id).await {
        if status == 200 {
            if let Ok(d) = serde_json::from_str::<Value>(&body) {
                let held: Vec<&Value> = d
                    .get("credits")
                    .and_then(|c| c.as_array())
                    .map(|a| {
                        a.iter()
                            .filter(|c| c.get("status").and_then(|s| s.as_str()) == Some("available"))
                            .collect()
                    })
                    .unwrap_or_default();
                if !held.is_empty() {
                    if let Some(e) = held
                        .iter()
                        .filter_map(|c| c.get("expires_at").and_then(|e| e.as_str()))
                        .min()
                    {
                        out["expires_at"] = json!(e);
                    }
                    if let Some(t) = held[0].get("title").and_then(|t| t.as_str()) {
                        out["title"] = json!(t);
                    }
                }
            }
        }
    }
    out
}

// "gemini-3-pro" | "claude-opus-4-6-thinking" | "gpt-oss-120b" -> "Gemini" / "Claude" / "GPT"
fn model_family(name: &str) -> &'static str {
    let n = name.to_lowercase();
    if n.starts_with("gemini") {
        "Gemini"
    } else if n.starts_with("claude") {
        "Claude"
    } else if n.starts_with("gpt") {
        "GPT"
    } else {
        "Other"
    }
}

fn pool_window(names: &[String], reset: Option<i64>) -> &'static str {
    if let Some(r) = reset {
        let now = Utc::now().timestamp();
        if r - now > 20000 {
            return "Weekly";
        } else {
            return "5-hour";
        }
    }
    let has_3rd_party = names.iter().any(|n| {
        let l = n.to_lowercase();
        l.starts_with("claude") || l.starts_with("gpt")
    });
    if has_3rd_party {
        "5-hour"
    } else {
        "Weekly"
    }
}

fn pool_label(names: &[String], reset: Option<i64>) -> String {
    let mut fams: Vec<&str> = names.iter().map(|n| model_family(n)).collect();
    fams.sort();
    fams.dedup();
    let fam = match fams.as_slice() {
        [f] => (*f).to_string(),
        fs => fs.join(" + "),
    };
    let win = pool_window(names, reset);
    if names.len() > 1 {
        format!("{fam} ({win}) pool ×{}", names.len())
    } else {
        format!("{fam} ({win})")
    }
}

// ---------- Antigravity ----------
async fn ag_post(
    client: &reqwest::Client,
    path: &str,
    token: &str,
    body: &str,
) -> Result<(u16, String), String> {
    let post = |base: &str| {
        let url = format!("{base}{path}");
        async move {
            let r = client
                .post(url)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::USER_AGENT, AG_UA)
                .body(body.to_string())
                .send()
                .await
                .map_err(|e| format!("send: {e}"))?;
            let status = r.status().as_u16();
            let body = r.text().await.map_err(|e| format!("read body: {e}"))?;
            Ok::<(u16, String), String>((status, body))
        }
    };
    if let Ok(explicit) = std::env::var("ANTIGRAVITY_BASE_URL") {
        return post(&explicit).await;
    }
    // Daily host first (live quota pool); fall back to prod on transport
    // error, 403/404 (host not serving this account/endpoint) or 5xx.
    // 401 returns as-is so the caller can refresh the token.
    let res = post(AG_DAILY_BASE).await?;
    match res.0 {
        401 | 200 | 201 => Ok(res),
        403 | 404 | 500..=599 => post(AG_BASE).await,
        _ => Ok(res),
    }
}

async fn ag_refresh(client: &reqwest::Client, refresh: &str) -> Result<String, String> {
    let r = client
        .post("https://oauth2.googleapis.com/token")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(format!(
            "grant_type=refresh_token&client_id={}&client_secret={}&refresh_token={}",
            urlenc(AG_CLIENT_ID),
            urlenc(AG_CLIENT_SECRET),
            urlenc(refresh)
        ))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let body = r.text().await.unwrap_or_default();
    let v: Value = serde_json::from_str(&body).map_err(|e| format!("refresh parse: {e}"))?;
    v.get("access_token")
        .and_then(|t| t.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "refresh failed".into())
}

async fn collect_antigravity(cfg: &Cfg, client: &reqwest::Client) -> Value {
    let err = |e: String| json!({"provider":"antigravity","status":"error","error":e});
    let raw = match read_file(&cfg.antigravity_token_file) {
        Ok(r) => r,
        Err(e) => return err(e),
    };
    let v: Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => return err(format!("parse token file: {e}")),
    };
    let mut token = v
        .pointer("/token/access_token")
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .to_string();
    if token.is_empty() {
        return err("no access token".into());
    }

    let models_body = "{}";
    let (mut status, mut body) = match ag_post(client, "/v1internal:fetchAvailableModels", &token, models_body).await {
        Ok(x) => x,
        Err(e) => return err(e),
    };

    // 401 -> refresh once and retry (read-only; never write back to token file)
    if status == 401 {
        let refresh = v.pointer("/token/refresh_token").and_then(|t| t.as_str()).unwrap_or("");
        if refresh.is_empty() {
            return err("token expired, no refresh token".into());
        }
        token = match ag_refresh(client, refresh).await {
            Ok(t) => t,
            Err(e) => return err(format!("token refresh: {e}")),
        };
        match ag_post(client, "/v1internal:fetchAvailableModels", &token, models_body).await {
            Ok((s, b)) => {
                status = s;
                body = b;
            }
            Err(e) => return err(e),
        }
    }

    if status != 200 {
        let snippet: String = body.chars().take(160).collect();
        return err(format!("http {status}: {snippet}"));
    }
    let mv: Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => return err(format!("parse models: {e}")),
    };

    // plan/tier via loadCodeAssist (best-effort; ignore failures)
    let mut plan = Value::Null;
    if let Ok((s, b)) = ag_post(
        client,
        "/v1internal:loadCodeAssist",
        &token,
        r#"{"metadata":{"ideName":"antigravity","ideType":"ANTIGRAVITY","ideVersion":"1.0.0","pluginVersion":"0.1","platform":"LINUX_AMD64","updateChannel":"stable","pluginType":"GEMINI"},"mode":"FULL_ELIGIBILITY_CHECK"}"#,
    )
    .await
    {
        if s == 200 {
            if let Ok(lv) = serde_json::from_str::<Value>(&b) {
                plan = lv
                    .pointer("/paidTier/id")
                    .or_else(|| lv.pointer("/currentTier/id"))
                    .cloned()
                    .unwrap_or(Value::Null);
            }
        }
    }

    // Extract models grouped by family from fetchAvailableModels
    // deprecatedModelIds maps old model ids to replacements (e.g. gemini-3.1-pro-high ->
    // gemini-pro-agent are the same model); canonicalize and dedup so pools count real models
    let mut deprec_map: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    if let Some(dep) = mv.get("deprecatedModelIds").and_then(|d| d.as_object()) {
        for (old, info) in dep {
            if let Some(new_id) = info.get("newModelId").and_then(|n| n.as_str()) {
                deprec_map.insert(old.clone(), new_id.to_string());
            }
        }
    }
    let canonical = |id: &str| -> String {
        let mut cur = id.to_string();
        for _ in 0..5 {
            match deprec_map.get(&cur) {
                Some(next) => cur = next.clone(),
                None => break,
            }
        }
        cur
    };
    let mut gemini_models = Vec::new();
    let mut claude_gpt_models = Vec::new();
    let mut other_models = Vec::new();
    if let Some(models) = mv.get("models").and_then(|m| m.as_object()) {
        for (name, info) in models {
            let is_tiered = name.ends_with("-tiered");
            if !is_tiered && info.get("displayName").and_then(|d| d.as_str()).map_or(true, |d| d.is_empty()) {
                continue; // skip internal/unnamed entries (chat_*, tab_*)
            }
            let clean_name = if is_tiered {
                name.strip_suffix("-tiered").unwrap_or(name)
            } else {
                name
            };
            let cname = canonical(clean_name);
            let fam = model_family(&cname);
            if fam == "Gemini" {
                if !gemini_models.contains(&cname) {
                    gemini_models.push(cname);
                }
            } else if fam == "Claude" || fam == "GPT" {
                if !claude_gpt_models.contains(&cname) {
                    claude_gpt_models.push(cname);
                }
            } else {
                if !other_models.contains(&cname) {
                    other_models.push(cname);
                }
            }
        }
    }
    gemini_models.sort();
    claude_gpt_models.sort();
    other_models.sort();

    let mut limits: Vec<Value> = Vec::new();

    // Prefer retrieveUserQuotaSummary: provides per-window 5h and weekly quota buckets
    if let Ok((qs, qb)) = ag_post(client, "/v1internal:retrieveUserQuotaSummary", &token, "{}").await {
        if qs == 200 {
            if let Ok(qv) = serde_json::from_str::<Value>(&qb) {
                if let Some(groups) = qv.get("groups").and_then(|g| g.as_array()) {
                    let mut sorted_groups = groups.clone();
                    sorted_groups.sort_by_key(|g| {
                        let name = g.get("displayName").and_then(|d| d.as_str()).unwrap_or("").to_lowercase();
                        if name.contains("gemini") {
                            0
                        } else if name.contains("claude") || name.contains("gpt") {
                            1
                        } else {
                            2
                        }
                    });

                    for group in sorted_groups {
                        let gname = group.get("displayName").and_then(|d| d.as_str()).unwrap_or("");
                        let gname_lower = gname.to_lowercase();

                        let (fam, gmodels) = if gname_lower.contains("gemini") {
                            ("Gemini", &gemini_models)
                        } else if gname_lower.contains("claude") || gname_lower.contains("gpt") {
                            ("Claude + GPT", &claude_gpt_models)
                        } else {
                            (gname, &other_models)
                        };

                        if let Some(buckets) = group.get("buckets").and_then(|b| b.as_array()) {
                            let mut sorted_buckets = buckets.clone();
                            sorted_buckets.sort_by_key(|b| {
                                let w = b.get("window").and_then(|w| w.as_str()).unwrap_or("");
                                match w {
                                    "5h" => 0,
                                    "weekly" => 1,
                                    _ => 2,
                                }
                            });

                            for b in sorted_buckets {
                                let window = b.get("window").and_then(|w| w.as_str()).unwrap_or("");
                                let (win_label, win_secs) = match window {
                                    "5h" => ("5-hour", Some(18000)),
                                    "weekly" => ("Weekly", Some(604800)),
                                    _ => (window, None),
                                };

                                let label = if gmodels.len() > 1 {
                                    format!("{fam} ({win_label}) pool ×{}", gmodels.len())
                                } else {
                                    format!("{fam} ({win_label})")
                                };

                                let rem_opt = b.get("remainingFraction").and_then(|f| f.as_f64());
                                let (pct, rem_pct) = match rem_opt {
                                    Some(rem) => {
                                        let p = ((1.0 - rem) * 100.0).clamp(0.0, 100.0);
                                        (p, (100.0 - p).max(0.0))
                                    }
                                    None => (100.0, 0.0),
                                };

                                let resets_at = b
                                    .get("resetTime")
                                    .and_then(|t| t.as_str())
                                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                                    .map(|dt| dt.timestamp() as f64);

                                limits.push(json!({
                                    "label": label,
                                    "models": gmodels,
                                    "used_percent": pct,
                                    "remaining_percent": rem_pct,
                                    "resets_at": resets_at,
                                    "window_seconds": win_secs,
                                }));
                            }
                        }
                    }
                }
            }
        }
    }

    // Collect models covered by retrieveUserQuotaSummary
    let covered_models: std::collections::HashSet<String> = limits
        .iter()
        .filter_map(|l| l.get("models").and_then(|m| m.as_array()))
        .flatten()
        .filter_map(|v| v.as_str().map(|s| s.to_string()))
        .collect();

    // Group any models not covered by retrieveUserQuotaSummary (e.g. Gemini 1P models)
    // using quotaInfo from fetchAvailableModels
    let mut pools: std::collections::HashMap<u64, (Vec<String>, Option<i64>)> =
        std::collections::HashMap::new();
    let mut unknown: Vec<String> = Vec::new();
    let mut exhausted: Vec<String> = Vec::new();
    let mut exhausted_reset: Option<i64> = None;
    if let Some(models) = mv.get("models").and_then(|m| m.as_object()) {
        for (name, info) in models {
            let is_tiered = name.ends_with("-tiered");
            if !is_tiered && info.get("displayName").and_then(|d| d.as_str()).map_or(true, |d| d.is_empty()) {
                continue; // skip internal/unnamed entries (chat_*, tab_*)
            }
            let clean_name = if is_tiered {
                name.strip_suffix("-tiered").unwrap_or(name)
            } else {
                name
            };
            let cname = canonical(clean_name);
            if covered_models.contains(&cname) {
                continue;
            }
            let qi = match info.get("quotaInfo") {
                Some(q) if !q.is_null() => q,
                _ => {
                    if !unknown.contains(&cname) {
                        unknown.push(cname);
                    }
                    continue;
                }
            };
            let rem = match qi.get("remainingFraction").and_then(|f| f.as_f64()) {
                Some(r) => r,
                None => {
                    if !exhausted.contains(&cname) {
                        exhausted.push(cname);
                    }
                    let reset = qi
                        .get("resetTime")
                        .and_then(|t| t.as_str())
                        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                        .map(|dt| dt.timestamp());
                    if let Some(r) = reset {
                        exhausted_reset = Some(r);
                    }
                    continue;
                }
            };
            let reset = qi
                .get("resetTime")
                .and_then(|t| t.as_str())
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.timestamp());
            let entry = pools.entry(rem.to_bits()).or_default();
            if !entry.0.contains(&cname) {
                entry.0.push(cname);
            }
            if reset.is_some() {
                entry.1 = reset;
            }
        }
    }
    let mut fallback_limits: Vec<Value> = pools
        .into_iter()
        .map(|(bits, (mut names, reset))| {
            names.sort();
            let rem = f64::from_bits(bits);
            let pct = ((1.0 - rem) * 100.0).clamp(0.0, 100.0);
            let label = pool_label(&names, reset);
            let win_secs = match pool_window(&names, reset) {
                "5-hour" => Some(18000),
                "Weekly" => Some(604800),
                _ => None,
            };
            json!({
                "label": label,
                "models": names,
                "used_percent": pct,
                "remaining_percent": (100.0 - pct).max(0.0),
                "resets_at": reset.map(|r| r as f64),
                "window_seconds": win_secs,
            })
        })
        .collect();
    fallback_limits.sort_by(|a, b| {
        let av = a.get("used_percent").and_then(|x| x.as_f64()).unwrap_or(0.0);
        let bv = b.get("used_percent").and_then(|x| x.as_f64()).unwrap_or(0.0);
        bv.partial_cmp(&av).unwrap_or(std::cmp::Ordering::Equal)
    });
    if !exhausted.is_empty() {
        exhausted.sort();
        let label = pool_label(&exhausted, exhausted_reset);
        fallback_limits.push(json!({
            "label": format!("{label} — limit hit"),
            "models": exhausted,
            "used_percent": 100.0,
            "remaining_percent": 0.0,
            "resets_at": exhausted_reset.map(|r| r as f64),
        }));
    }
    if !unknown.is_empty() {
        unknown.sort();
        fallback_limits.push(json!({
            "label": format!("no quota data ×{}", unknown.len()),
            "models": unknown,
            "unknown": true,
            "used_percent": null,
        }));
    }

    limits.extend(fallback_limits);

    // Sort limits: Gemini first (0), Claude + GPT (1), others (2)
    // Within family: 5-hour before Weekly
    limits.sort_by_key(|lim| {
        let label = lim.get("label").and_then(|l| l.as_str()).unwrap_or("").to_lowercase();
        let fam_rank = if label.contains("gemini") {
            0
        } else if label.contains("claude") || label.contains("gpt") {
            1
        } else {
            2
        };
        let win_rank = if label.contains("5-hour") {
            0
        } else if label.contains("weekly") {
            1
        } else {
            2
        };
        (fam_rank, win_rank)
    });

    if limits.is_empty() {
        return err("no models with quota returned".into());
    }
    json!({"provider":"antigravity","status":"ok","plan":plan,"limits":limits})
}

// ---------- Muse Code (Meta) ----------
// Same request the `muse` CLI makes at startup to mint its inference key:
// POST https://api.meta.ai/muse-code/key with the OAuth device-code token
// (`dca:...`) from providers.meta.access_token in ~/.config/muse/auth.json.
// Only allowlisted quota fields are parsed; the minted api_key in the response
// is discarded and never logged, stored, or rendered.
async fn collect_muse(cfg: &Cfg, client: &reqwest::Client) -> Value {
    let err = |e: String| json!({"provider":"muse","status":"error","error":e});
    let raw = match read_file(&cfg.muse_auth_file) {
        Ok(r) => r,
        Err(e) => return err(e),
    };
    let v: Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => return err(format!("parse auth: {e}")),
    };
    let token = v
        .pointer("/providers/meta/access_token")
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .to_string();
    if token.is_empty() {
        return err("no access token (run `muse login`)".into());
    }
    let email = v
        .pointer("/providers/meta/user_email")
        .and_then(|e| e.as_str())
        .unwrap_or("")
        .to_string();

    let resp = client
        .post(MUSE_KEY_URL)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .header("x-api-version", "1.0.0")
        .header(header::USER_AGENT, ZAI_UA)
        .body("{}")
        .send()
        .await;
    let resp = match resp {
        Ok(r) => r,
        Err(e) => return err(e.to_string()),
    };
    let status = resp.status().as_u16();
    let body = match resp.text().await {
        Ok(b) => b,
        Err(e) => return err(format!("read body: {e}")),
    };
    if status != 200 {
        let snippet: String = body.chars().take(160).collect();
        return err(format!("http {status}: {snippet}"));
    }
    let v: Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(_) => return err("bad json".into()),
    };
    if let Some(error_val) = v.get("error") {
        return err(format!("{error_val}"));
    }
    let plan = v
        .get("subs_tier_name")
        .and_then(|s| s.as_str())
        .map(|s| s.to_string());
    let active = v.get("is_subs_active").and_then(|b| b.as_bool()).unwrap_or(false);
    if !active {
        return json!({"provider":"muse","status":"error","plan":plan,"email":email,"error":"no active subscription"});
    }
    // Meta omits subs_usage while the window is idle — active plan with no
    // quota block is an empty reading, not an error and never 0% invented.
    let su = match v.get("subs_usage") {
        Some(su) if su.is_object() => su,
        _ => return json!({"provider":"muse","status":"ok","plan":plan,"email":email,"limits":[]}),
    };
    let mut limits = Vec::new();
    if let Some(w) = su.get("window") {
        if let Some(pct) = w.get("used_percent").and_then(|p| p.as_f64()) {
            let secs = w
                .get("window_duration_mins")
                .and_then(|m| m.as_i64())
                .map(|m| m * 60);
            let resets_at = w
                .get("resets_at")
                .and_then(|r| r.as_i64().map(|x| x as f64).or_else(|| r.as_f64()));
            limits.push(json!({
                "label": "5-hour",
                "used_percent": pct,
                "remaining_percent": (100.0 - pct).max(0.0),
                "window_seconds": secs,
                "resets_at": resets_at,
            }));
        }
    }
    if let Some(wk) = su.get("weekly") {
        if let Some(pct) = wk.get("used_percent").and_then(|p| p.as_f64()) {
            let resets_at = wk
                .get("resets_at")
                .and_then(|r| r.as_i64().map(|x| x as f64).or_else(|| r.as_f64()));
            limits.push(json!({
                "label": "Weekly",
                "used_percent": pct,
                "remaining_percent": (100.0 - pct).max(0.0),
                "window_seconds": 604800,
                "resets_at": resets_at,
            }));
        }
    }
    if limits.is_empty() {
        return err("no usage windows found in response".into());
    }
    json!({"provider":"muse","status":"ok","plan":plan,"email":email,"limits":limits})
}

// ---------- aggregate ----------
async fn collect_all(state: &AppState, claude_mode: ClaudeMode) -> Value {
    let (zai, opencode_go, claude, codex, antigravity, muse) = tokio::join!(
        collect_zai(&state.cfg, &state.client),
        collect_opencode_go(&state.cfg, &state.client),
        collect_claude(&state.cfg, &state.client, claude_mode),
        collect_codex(&state.cfg, &state.client),
        collect_antigravity(&state.cfg, &state.client),
        collect_muse(&state.cfg, &state.client)
    );
    let mut providers = vec![zai, opencode_go];
    providers.extend(claude);
    providers.extend([codex, antigravity, muse]);
    let any_error = providers.iter().any(|p| !is_ok_status(p));
    json!({
        "collected": Utc::now().to_rfc3339(),
        "ts": Utc::now().timestamp(),
        "any_error": any_error,
        "claude_mode": claude_mode.as_str(),
        "claude_multi_available": state.cfg.claude_swap_bin.is_some(),
        "providers": providers,
    })
}

const HISTORY_LEN: usize = 60;

fn history_snapshot(data: &Value) -> Value {
    // trim each provider to {provider, account, limits:[{label,used_percent}]}
    let providers = data
        .get("providers")
        .and_then(|p| p.as_array())
        .map(|arr| {
            arr.iter()
                .map(|p| {
                    let limits = p
                        .get("limits")
                        .and_then(|l| l.as_array())
                        .map(|ls| {
                            ls.iter()
                                .filter_map(|l| {
                                    Some(json!({
                                        "label": l.get("label").and_then(|x| x.as_str()).unwrap_or(""),
                                        "used_percent": l.get("used_percent").and_then(|x| x.as_f64()).unwrap_or(0.0),
                                    }))
                                })
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    json!({
                        "provider": p.get("provider").and_then(|x| x.as_str()).unwrap_or(""),
                        "account": p.get("account").and_then(|x| x.as_str()).unwrap_or(""),
                        "source": p.get("source").and_then(|x| x.as_str()).unwrap_or(""),
                        "limits": limits,
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    json!({
        "ts": data.get("ts").and_then(|x| x.as_i64()).unwrap_or(0),
        "providers": providers,
    })
}

async fn background_loop(state: AppState) {
    loop {
        let _guard = state.collect_lock.lock().await;
        let mode = *state.claude_mode.read().await;
        let data = collect_all(&state, mode).await;
        store_snapshot(&state, data).await;
        drop(_guard);
        tokio::time::sleep(state.cfg.interval).await;
    }
}

async fn store_snapshot(state: &AppState, data: Value) {
    {
        let mut h = state.history.write().await;
        h.push(history_snapshot(&data));
        if h.len() > HISTORY_LEN {
            h.remove(0);
        }
    }
    *state.data.write().await = data;
}

// ---------- HTTP ----------
async fn index() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        HTML,
    )
}

async fn data_handler(State(state): State<AppState>) -> Json<Value> {
    Json(state.data.read().await.clone())
}

async fn history_handler(State(state): State<AppState>) -> Json<Value> {
    Json(json!(state.history.read().await.clone()))
}

async fn claude_mode_handler(
    State(state): State<AppState>,
    Json(payload): Json<Value>,
) -> (StatusCode, Json<Value>) {
    let mode = match payload.get("mode").and_then(|m| m.as_str()) {
        Some("direct") => ClaudeMode::Direct,
        Some("multi") => ClaudeMode::Multi,
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error":"mode must be 'direct' or 'multi'"})),
            );
        }
    };
    if mode == ClaudeMode::Multi && state.cfg.claude_swap_bin.is_none() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":"claude-swap not found; set CLAUDE_SWAP_BIN"})),
        );
    }

    let _guard = state.collect_lock.lock().await;
    *state.claude_mode.write().await = mode;
    let claude = collect_claude(&state.cfg, &state.client, mode).await;
    let mut data = state.data.read().await.clone();
    if let Some(providers) = data.get_mut("providers").and_then(|p| p.as_array_mut()) {
        providers.retain(|p| p.get("provider").and_then(|v| v.as_str()) != Some("claude"));
        providers.extend(claude);
        let any_error = providers.iter().any(|p| !is_ok_status(p));
        data["any_error"] = json!(any_error);
    }
    data["claude_mode"] = json!(mode.as_str());
    *state.data.write().await = data;
    (StatusCode::OK, Json(json!({"mode":mode.as_str()})))
}

#[tokio::main]
async fn main() {
    let cfg = Cfg::from_env();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .pool_max_idle_per_host(0)
        .build()
        .expect("build client");

    let data = Arc::new(RwLock::new(json!({"collected":null,"ts":0,"any_error":true,"providers":[]})));
    let history = Arc::new(RwLock::new(Vec::new()));
    let state = AppState {
        cfg,
        data,
        history,
        claude_mode: Arc::new(RwLock::new(ClaudeMode::Direct)),
        collect_lock: Arc::new(Mutex::new(())),
        client,
    };

    // first collect before serving
    let boot_state = state.clone();
    let boot = collect_all(&boot_state, ClaudeMode::Direct).await;
    store_snapshot(&state, boot).await;

    // background refresh
    let bg_state = state.clone();
    tokio::spawn(background_loop(bg_state));

    let addr: std::net::SocketAddr = state.cfg.listen.parse().unwrap_or_else(|_| "0.0.0.0:8000".parse().unwrap());

    let app = Router::new()
        .route("/", get(index))
        .route("/data.json", get(data_handler))
        .route("/history.json", get(history_handler))
        .route("/claude/mode", post(claude_mode_handler))
        .with_state(state);

    eprintln!("aidash listening on http://{addr}");
    let listener = tokio::net::TcpListener::bind(addr).await.expect("bind");
    axum::serve(listener, app).await.expect("server");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_labelled_claude_creds() {
        let creds = parse_claude_creds(" personal=/a/.credentials.json , work=/b/.credentials.json ");
        assert_eq!(creds.len(), 2);
        assert_eq!(creds[0].label.as_deref(), Some("personal"));
        assert_eq!(creds[0].path, PathBuf::from("/a/.credentials.json"));
        assert_eq!(creds[1].label.as_deref(), Some("work"));
        assert_eq!(creds[1].path, PathBuf::from("/b/.credentials.json"));
    }

    #[test]
    fn parses_unlabelled_claude_creds_and_dedupes_paths() {
        let creds = parse_claude_creds("/a.json,,  ,/b.json,/a.json");
        assert_eq!(creds.len(), 2);
        assert!(creds.iter().all(|c| c.label.is_none()));
        assert_eq!(creds[0].path, PathBuf::from("/a.json"));
        assert_eq!(creds[1].path, PathBuf::from("/b.json"));
    }

    #[test]
    fn derives_claude_account_labels_from_paths() {
        assert_eq!(
            claude_path_label(Path::new("/home/me/.claude-work/.credentials.json")),
            "claude-work"
        );
        assert_eq!(
            claude_path_label(Path::new("/home/me/.claude/.credentials.json")),
            "account"
        );
        assert_eq!(claude_path_label(Path::new("/home/me/work-claude.json")), "work-claude");
    }

    #[test]
    fn maps_claude_swap_accounts_to_providers() {
        let v = json!({
            "schemaVersion": 1,
            "activeAccountNumber": 3,
            "accounts": [
                {
                    "number": 2,
                    "email": "two@example.com",
                    "active": false,
                    "usageStatus": "ok",
                    "usage": {
                        "fiveHour": { "pct": 19.0, "resetsAt": "2026-10-22T22:50:00.061742+00:00" },
                        "sevenDay": { "pct": 96.0, "resetsAt": "2026-10-26T20:00:00.061764+00:00" },
                        "scoped": [
                            { "name": "Fable", "pct": 0.0, "resetsAt": "2026-10-26T20:00:00+00:00" }
                        ]
                    }
                },
                {
                    "number": 3,
                    "email": "three@example.com",
                    "active": true,
                    "usageStatus": "token_expired",
                    "usage": null
                }
            ]
        });
        let rows = claude_swap_providers(&v).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["provider"], "claude");
        assert_eq!(rows[0]["source"], "cswap");
        assert_eq!(rows[0]["account"], "two@example.com");
        assert_eq!(rows[0]["status"], "ok");
        let limits = rows[0]["limits"].as_array().unwrap();
        assert_eq!(limits.len(), 3);
        assert_eq!(limits[0]["label"], "5-hour");
        assert_eq!(limits[0]["used_percent"], 19.0);
        assert!(limits[0]["resets_at"].as_f64().is_some());
        assert_eq!(limits[2]["label"], "Fable (7-day)");
        assert_eq!(rows[1]["status"], "error");
        assert_eq!(rows[1]["active"], true);
        assert!(rows[1]["error"].as_str().unwrap().contains("token expired"));
    }
}
