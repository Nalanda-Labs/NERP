//! A LiveKit media server for calls: access tokens for browsers, a reachability check, and
//! the Egress API for recording (room composite recordings to MP4 files).
//!
//! LiveKit's server API is Twirp (JSON over HTTP POST) authorised with a JWT signed with the
//! API secret; browsers get their own short JWT to join one room.

use std::{
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde_json::{json, Value};

use crate::{config::LiveKitConfig, state::AppState};

/// The LiveKit room of a NERP call.
pub fn room_name(call: uuid::Uuid) -> String {
    format!("nerp-call-{call}")
}

fn sign(cfg: &LiveKitConfig, sub: &str, extra: Value, ttl: Duration) -> String {
    let now = chrono::Utc::now().timestamp();
    let mut claims = json!({ "iss": cfg.api_key, "sub": sub, "nbf": now - 10, "exp": now + ttl.as_secs() as i64 });
    if let (Some(c), Value::Object(e)) = (claims.as_object_mut(), extra) {
        c.extend(e);
    }
    jsonwebtoken::encode(&Header::new(Algorithm::HS256), &claims, &EncodingKey::from_secret(cfg.api_secret.as_bytes())).unwrap_or_default()
}

/// A browser's token to join (and publish to) one room.
pub fn join_token(cfg: &LiveKitConfig, room: &str, identity: &str, name: &str, metadata: &Value) -> String {
    sign(
        cfg,
        identity,
        json!({
            "name": name,
            "metadata": metadata.to_string(),
            "video": { "room": room, "roomJoin": true, "canPublish": true, "canSubscribe": true, "canPublishData": true },
        }),
        Duration::from_secs(6 * 3600),
    )
}

fn http() -> Result<reqwest::Client, String> {
    crate::crm::email::oauth::http()
}

/// Whether the LiveKit server answers (checked at most every 5 seconds).
pub async fn reachable(state: &AppState) -> bool {
    let Some(cfg) = &state.config.livekit else { return false };
    static LAST: OnceLock<Mutex<Option<(Instant, bool)>>> = OnceLock::new();
    let cache = LAST.get_or_init(|| Mutex::new(None));
    if let Some((at, ok)) = *cache.lock().unwrap() {
        if at.elapsed() < Duration::from_secs(5) {
            return ok;
        }
    }
    let ok = match http() {
        Ok(c) => matches!(c.get(format!("{}/", cfg.api_url)).timeout(Duration::from_secs(3)).send().await, Ok(r) if r.status().is_success()),
        Err(_) => false,
    };
    if !ok {
        tracing::warn!(url = %cfg.api_url, "the LiveKit server does not answer; calls connect directly");
    }
    *cache.lock().unwrap() = Some((Instant::now(), ok));
    ok
}

/// Whether calls through LiveKit can be recorded (a recordings folder is configured).
pub fn can_record(state: &AppState) -> bool {
    state.config.livekit.as_ref().is_some_and(|c| c.recordings_dir.is_some())
}

/// A Twirp call to the LiveKit server API.
async fn twirp(cfg: &LiveKitConfig, service: &str, method: &str, grant: Value, body: Value) -> Result<Value, String> {
    let token = sign(cfg, "nerp", json!({ "video": grant }), Duration::from_secs(600));
    let res = http()?
        .post(format!("{}/twirp/livekit.{service}/{method}", cfg.api_url))
        .bearer_auth(token)
        .json(&body)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| format!("cannot reach the video server: {e}"))?;
    let status = res.status();
    let v: Value = res.json().await.unwrap_or_default();
    if !status.is_success() {
        let msg = v["msg"].as_str().unwrap_or(status.as_str());
        return Err(if msg.contains("egress") && (msg.contains("not connected") || msg.contains("no response") || msg.contains("not available")) {
            "the recording service (LiveKit Egress) is not running".to_string()
        } else {
            format!("the video server refused: {msg}")
        });
    }
    Ok(v)
}

/// Starts recording a room (everyone, in a grid, with mixed sound) to an MP4 file named
/// `file_name` in the recordings folder. Returns Egress's id.
pub async fn start_recording(state: &AppState, room: &str, file_name: &str) -> Result<String, String> {
    let cfg = state.config.livekit.as_ref().ok_or("no video server is configured")?;
    let dir = cfg.egress_dir.as_deref().ok_or("recording is not configured (LIVEKIT_RECORDINGS_DIR)")?;
    let v = twirp(
        cfg,
        "Egress",
        "StartRoomCompositeEgress",
        json!({ "roomRecord": true }),
        json!({
            "room_name": room,
            "layout": "grid",
            "file_outputs": [{ "file_type": "MP4", "filepath": format!("{}/{file_name}", dir.trim_end_matches('/')) }],
        }),
    )
    .await?;
    field(&v, "egress_id", "egressId").and_then(|x| x.as_str()).map(String::from).ok_or_else(|| "the video server did not start the recording".into())
}

pub async fn stop_recording(state: &AppState, egress_id: &str) -> Result<(), String> {
    let cfg = state.config.livekit.as_ref().ok_or("no video server is configured")?;
    match twirp(cfg, "Egress", "StopEgress", json!({ "roomRecord": true }), json!({ "egress_id": egress_id })).await {
        Ok(_) => Ok(()),
        // Already finished (e.g. everyone left).
        Err(e) if e.contains("cannot be stopped") || e.contains("not found") || e.contains("EGRESS_COMPLETE") || e.contains("ENDING") => Ok(()),
        Err(e) => Err(e),
    }
}

/// Where a recording is: (status, error, duration in seconds). None: Egress doesn't know it.
pub async fn recording_status(state: &AppState, egress_id: &str) -> Result<Option<(String, Option<String>, Option<i32>)>, String> {
    let cfg = state.config.livekit.as_ref().ok_or("no video server is configured")?;
    let v = twirp(cfg, "Egress", "ListEgress", json!({ "roomRecord": true }), json!({ "egress_id": egress_id })).await?;
    let Some(item) = v["items"].as_array().and_then(|a| a.first()) else { return Ok(None) };
    let status = match &item["status"] {
        Value::String(s) => s.clone(),
        // Numeric enums: the order of livekit.EgressStatus.
        Value::Number(n) => ["EGRESS_STARTING", "EGRESS_ACTIVE", "EGRESS_ENDING", "EGRESS_COMPLETE", "EGRESS_FAILED", "EGRESS_ABORTED", "EGRESS_LIMIT_REACHED"]
            .get(n.as_u64().unwrap_or(0) as usize)
            .unwrap_or(&"EGRESS_STARTING")
            .to_string(),
        _ => "EGRESS_STARTING".into(),
    };
    let error = item["error"].as_str().filter(|e| !e.is_empty()).map(String::from);
    let files = field(item, "file_results", "fileResults").and_then(|f| f.as_array().cloned()).unwrap_or_default();
    let duration = files.first().and_then(|f| {
        let d = &f["duration"];
        d.as_i64().or_else(|| d.as_str().and_then(|s| s.parse().ok()))
    });
    Ok(Some((status, error, duration.map(|ns| (ns / 1_000_000_000) as i32))))
}

fn field<'a>(v: &'a Value, snake: &str, camel: &str) -> Option<&'a Value> {
    v.get(snake).filter(|x| !x.is_null()).or_else(|| v.get(camel).filter(|x| !x.is_null()))
}
