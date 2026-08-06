use axum::{extract::State, http::header, response::IntoResponse, routing::get, Json, Router};
use chrono::Utc;
use serde_json::{json, Value};
use std::env;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

const HTML: &str = include_str!("../index.html");
const CODEX_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const ZAI_UA: &str = "aidash/0.1";
const CLAUDE_UA: &str = "claude-code/1.0.0";

#[derive(Clone)]
struct Cfg {
    zai_key_file: PathBuf,
    claude_cred_file: PathBuf,
    codex_auth_file: PathBuf,
    listen: String,
    interval: Duration,
}

#[derive(Clone)]
struct AppState {
    cfg: Cfg,
    data: Arc<RwLock<Value>>,
    history: Arc<RwLock<Vec<Value>>>,
    client: reqwest::Client,
}

impl Cfg {
    fn from_env() -> Self {
        let home = env::var("HOME").unwrap_or_else(|_| "/root".into());
        Self {
            zai_key_file: PathBuf::from(env::var("ZAI_KEY_FILE").unwrap_or_else(|_| format!("{home}/aidash/zai.key"))),
            claude_cred_file: PathBuf::from(
                env::var("CLAUDE_CRED_FILE").unwrap_or_else(|_| format!("{home}/.claude/.credentials.json")),
            ),
            codex_auth_file: PathBuf::from(
                env::var("CODEX_AUTH_FILE").unwrap_or_else(|_| format!("{home}/.codex/auth.json")),
            ),
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
    json!({"provider":"z.ai","status":"ok","plan":level,"limits":out})
}

// ---------- Claude ----------
fn claude_ts(v: &Value) -> Option<f64> {
    let s = v.as_str()?;
    let s = s.trim_end_matches('Z');
    chrono::DateTime::parse_from_rfc3339(s).ok().map(|dt| dt.timestamp() as f64)
}

async fn collect_claude(cfg: &Cfg, client: &reqwest::Client) -> Value {
    let cred = match read_file(&cfg.claude_cred_file) {
        Ok(c) => c,
        Err(e) => return json!({"provider":"claude","status":"error","error":e}),
    };
    let token = match serde_json::from_str::<Value>(&cred) {
        Ok(v) => v
            .pointer("/claudeAiOauth/accessToken")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string(),
        Err(e) => return json!({"provider":"claude","status":"error","error":format!("parse creds: {e}")}),
    };
    if token.is_empty() {
        return json!({"provider":"claude","status":"error","error":"no access token"});
    }
    let resp = client
        .get("https://api.anthropic.com/api/oauth/usage")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::USER_AGENT, CLAUDE_UA)
        .send()
        .await;
    let body = match resp {
        Ok(r) => r.text().await.unwrap_or_default(),
        Err(e) => return json!({"provider":"claude","status":"error","error":e.to_string()}),
    };
    let v: Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(_) => return json!({"provider":"claude","status":"error","error":"bad json"}),
    };
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
    json!({"provider":"claude","status":"ok","plan":null,"limits":limits})
}

// ---------- Codex ----------
async fn codex_get(client: &reqwest::Client, token: &str, account_id: &str) -> Result<String, String> {
    let mut req = client
        .get("https://chatgpt.com/backend-api/wham/usage")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::ACCEPT, "application/json")
        .header(header::ORIGIN, "https://chatgpt.com");
    if !account_id.is_empty() {
        req = req.header("ChatGPT-Account-Id", account_id);
    }
    let r = req.send().await.map_err(|e| e.to_string())?;
    Ok(r.text().await.unwrap_or_default())
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

    let mut body = codex_get(client, &token, &account_id).await.unwrap_or_default();
    let mut parsed: Result<Value, _> = serde_json::from_str(&body);

    // 401 -> try refresh (read-only; never write back to auth file)
    if parsed.as_ref().ok().and_then(|v| v.get("error")).is_some() {
        if std::env::var("CODEX_DISABLE_REFRESH").ok().as_deref() == Some("1") {
            return json!({"provider":"codex","status":"error","error":"token invalid","limits":[]});
        }
        let refresh = v
            .pointer("/tokens/refresh_token")
            .and_then(|t| t.as_str())
            .unwrap_or("");
        if !refresh.is_empty() {
            match codex_refresh(client, refresh).await {
                Ok((at, _, _)) => {
                    token = at;
                    body = codex_get(client, &token, &account_id).await.unwrap_or_default();
                    parsed = serde_json::from_str(&body);
                }
                Err(e) => return json!({"provider":"codex","status":"error","error":e}),
            }
        }
    }

    let v: Value = match parsed {
        Ok(v) => v,
        Err(_) => return json!({"provider":"codex","status":"error","error":"bad json"}),
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
        limits.push(json!({
            "label": "primary",
            "used_percent": pct,
            "remaining_percent": (100.0 - pct).max(0.0),
            "window_seconds": pw.get("limit_window_seconds"),
            "resets_at": pw.get("reset_at"),
        }));
    }
    if let Some(sw) = rl.get("secondary_window").filter(|s| !s.is_null()) {
        let pct = sw.get("used_percent").and_then(|p| p.as_f64()).unwrap_or(0.0);
        limits.push(json!({
            "label": "secondary",
            "used_percent": pct,
            "remaining_percent": (100.0 - pct).max(0.0),
            "window_seconds": sw.get("limit_window_seconds"),
            "resets_at": sw.get("reset_at"),
        }));
    }
    if let Some(extra) = v.get("additional_rate_limits").and_then(|a| a.as_array()) {
        for m in extra {
            let name = m.get("limit_name").and_then(|n| n.as_str()).unwrap_or("model");
            if let Some(pw) = m.pointer("/rate_limit/primary_window") {
                let pct = pw.get("used_percent").and_then(|p| p.as_f64()).unwrap_or(0.0);
                limits.push(json!({
                    "label": format!("model:{name}"),
                    "used_percent": pct,
                    "remaining_percent": (100.0 - pct).max(0.0),
                    "resets_at": pw.get("reset_at"),
                }));
            }
        }
    }
    json!({"provider":"codex","status":"ok","plan":plan,"email":email,"limits":limits})
}

// ---------- aggregate ----------
async fn collect_all(state: &AppState) -> Value {
    let (zai, claude, codex) = tokio::join!(
        collect_zai(&state.cfg, &state.client),
        collect_claude(&state.cfg, &state.client),
        collect_codex(&state.cfg, &state.client)
    );
    let providers = vec![zai, claude, codex];
    let any_error = providers.iter().any(|p| !is_ok_status(p));
    json!({
        "collected": Utc::now().to_rfc3339(),
        "ts": Utc::now().timestamp(),
        "any_error": any_error,
        "providers": providers,
    })
}

const HISTORY_LEN: usize = 60;

fn history_snapshot(data: &Value) -> Value {
    // trim each provider to {provider, limits:[{label,used_percent}]}
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
        let data = collect_all(&state).await;
        {
            let mut h = state.history.write().await;
            h.push(history_snapshot(&data));
            if h.len() > HISTORY_LEN {
                h.remove(0);
            }
        }
        *state.data.write().await = data;
        tokio::time::sleep(state.cfg.interval).await;
    }
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

#[tokio::main]
async fn main() {
    let cfg = Cfg::from_env();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .expect("build client");

    let data = Arc::new(RwLock::new(json!({"collected":null,"ts":0,"any_error":true,"providers":[]})));
    let history = Arc::new(RwLock::new(Vec::new()));
    let state = AppState {
        cfg,
        data,
        history,
        client,
    };

    // first collect before serving
    let boot_state = state.clone();
    let boot = collect_all(&boot_state).await;
    {
        let mut h = state.history.write().await;
        h.push(history_snapshot(&boot));
    }
    *state.data.write().await = boot;

    // background refresh
    let bg_state = state.clone();
    tokio::spawn(background_loop(bg_state));

    let addr: std::net::SocketAddr = state.cfg.listen.parse().unwrap_or_else(|_| "0.0.0.0:8000".parse().unwrap());

    let app = Router::new()
        .route("/", get(index))
        .route("/data.json", get(data_handler))
        .route("/history.json", get(history_handler))
        .with_state(state);

    eprintln!("aidash listening on http://{addr}");
    let listener = tokio::net::TcpListener::bind(addr).await.expect("bind");
    axum::serve(listener, app).await.expect("server");
}
