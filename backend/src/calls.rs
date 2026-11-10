//! Video calls: browsers talk to each other directly (WebRTC, every participant to every
//! other), and NERP only passes their setup messages along and keeps track of who is in.
//!
//! A call starts from a chat (its members are invited) or with chosen people. Invited people
//! get a ringing notice; each browser that joins gets a peer id, offers a connection to those
//! already in, and keeps a heartbeat going; one that falls silent is dropped. The call ends
//! when the last person leaves, and the chat gets a note with its length. Calls cross
//! networks only with a TURN server (WEBRTC_ICE_SERVERS); within one network they connect
//! directly.
//!
//! With a LiveKit media server configured (LIVEKIT_URL) and answering when a call starts, the
//! call goes through it instead: each browser sends its media once to LiveKit, which forwards
//! it (so calls can be bigger), and calls can be recorded with LiveKit Egress (an MP4 of
//! everyone in a grid, posted in the call's chat when it is ready).

use anyhow::Result;
use axum::{
    extract::{Path, State},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{
    auth::AuthUser,
    error::{AppError, AppResult},
    notify::{notify, NewNotification},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/calls", get(history).post(start))
        .route("/calls/{id}/controls", post(controls))
        .route("/calls/{id}", get(get_call))
        .route("/calls/{id}/join", post(join))
        .route("/calls/{id}/signal", post(signal))
        .route("/calls/{id}/heartbeat", post(heartbeat))
        .route("/calls/{id}/leave", post(leave))
        .route("/calls/{id}/decline", post(decline))
        .route("/calls/{id}/recording", post(start_recording).delete(stop_recording))
}

/// Most people in a call: every browser connects to every other without a media server.
const MAX_P2P: usize = 8;
const MAX_LIVEKIT: usize = 50;

/// A browser that hasn't sent a heartbeat for this long has left.
const SILENT_SECS: i64 = 45;

async fn invited(state: &AppState, call: Uuid) -> AppResult<Vec<Uuid>> {
    Ok(sqlx::query_scalar("SELECT user_id FROM call_invites WHERE call_id = $1").bind(call).fetch_all(&state.db).await?)
}

async fn check(state: &AppState, auth: &AuthUser, call: Uuid) -> AppResult<Vec<Uuid>> {
    let who = invited(state, call).await?;
    if !who.contains(&auth.id) {
        return Err(AppError::NotFound("call"));
    }
    Ok(who)
}

async fn name(state: &AppState, user: Uuid) -> String {
    sqlx::query_scalar("SELECT trim(first_name || ' ' || last_name) FROM users WHERE id = $1").bind(user).fetch_one(&state.db).await.unwrap_or_default()
}

/// Peers in the call now: (peer, user, name).
async fn peers(state: &AppState, call: Uuid) -> AppResult<Vec<(Uuid, Uuid, String)>> {
    Ok(sqlx::query_as(
        "SELECT p.peer_id, p.user_id, trim(u.first_name || ' ' || u.last_name) FROM call_peers p JOIN users u ON u.id = p.user_id
         WHERE p.call_id = $1 AND p.left_at IS NULL ORDER BY p.joined_at",
    )
    .bind(call)
    .fetch_all(&state.db)
    .await?)
}

async fn chat_note(state: &AppState, chat: Option<Uuid>, text: String) {
    if let Some(c) = chat {
        if let Err(e) = crate::im::post_message(state, c, None, &text, &[]).await {
            tracing::warn!(error = ?e, "a call note could not be posted");
        }
    }
}

#[derive(Deserialize)]
struct StartBody {
    chat_id: Option<Uuid>,
    #[serde(default)]
    users: Vec<Uuid>,
}

async fn start(State(state): State<AppState>, auth: AuthUser, Json(b): Json<StartBody>) -> AppResult<Json<Value>> {
    let mut who: Vec<Uuid> = match b.chat_id {
        Some(chat) => {
            let m: Vec<Uuid> = sqlx::query_scalar("SELECT user_id FROM im_members WHERE chat_id = $1").bind(chat).fetch_all(&state.db).await?;
            if !m.contains(&auth.id) {
                return Err(AppError::NotFound("chat"));
            }
            // A call already going on in this chat: join that one.
            let going: Option<Uuid> = sqlx::query_scalar(
                "SELECT c.id FROM calls c WHERE c.chat_id = $1 AND c.ended_at IS NULL AND EXISTS (SELECT 1 FROM call_peers p WHERE p.call_id = c.id AND p.left_at IS NULL)
                 ORDER BY c.started_at DESC LIMIT 1",
            )
            .bind(chat)
            .fetch_optional(&state.db)
            .await?;
            if let Some(id) = going {
                sqlx::query("INSERT INTO call_invites (call_id, user_id) SELECT $1, unnest($2::uuid[]) ON CONFLICT DO NOTHING").bind(id).bind(&m).execute(&state.db).await?;
                return get_call(State(state), auth, Path(id)).await;
            }
            m
        }
        None => {
            let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE id = ANY($1) AND is_active").bind(&b.users).fetch_one(&state.db).await?;
            if n as usize != b.users.len() || b.users.is_empty() {
                return Err(AppError::bad("call one or more active colleagues"));
            }
            let mut u = b.users.clone();
            u.push(auth.id);
            u
        }
    };
    who.sort();
    who.dedup();
    if who.len() < 2 {
        return Err(AppError::bad("a call needs someone else"));
    }
    // Through the media server when there is one and it answers.
    let media = if crate::livekit::reachable(&state).await { "livekit" } else { "p2p" };
    let max = if media == "livekit" { MAX_LIVEKIT } else { MAX_P2P };
    if who.len() > max {
        return Err(AppError::bad(format!("calls are for up to {max} people")));
    }
    let mut tx = state.db.begin().await?;
    let id: Uuid = sqlx::query_scalar("INSERT INTO calls (chat_id, started_by, media) VALUES ($1, $2, $3) RETURNING id")
        .bind(b.chat_id)
        .bind(auth.id)
        .bind(media)
        .fetch_one(&mut *tx)
        .await?;
    for u in &who {
        sqlx::query("INSERT INTO call_invites (call_id, user_id) VALUES ($1, $2)").bind(id).bind(u).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    let me = name(&state, auth.id).await;
    let others: Vec<Uuid> = who.iter().copied().filter(|u| *u != auth.id).collect();
    crate::realtime::publish(&state, Some(others.clone()), "call.ring", json!({ "call_id": id, "from": me })).await;
    notify(&state, &others, NewNotification { kind: "call", title: format!("{me} is calling"), body: None, link: Some(format!("/call/{id}")), actor: Some(auth.id) }).await;
    chat_note(&state, b.chat_id, format!("{} started a call", me.split_whitespace().next().unwrap_or(&me))).await;
    get_call(State(state), auth, Path(id)).await
}

async fn get_call(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    check(&state, &auth, id).await?;
    #[allow(clippy::type_complexity)]
    let (chat, started_at, ended_at, media, host, allow_audio, allow_video, allow_screen): (Option<Uuid>, DateTime<Utc>, Option<DateTime<Utc>>, String, Option<Uuid>, bool, bool, bool) =
        sqlx::query_as("SELECT chat_id, started_at, ended_at, media, started_by, allow_audio, allow_video, allow_screen FROM calls WHERE id = $1").bind(id).fetch_one(&state.db).await?;
    let people: Vec<(Uuid, String, Option<Uuid>, bool)> = sqlx::query_as(
        "SELECT u.id, trim(u.first_name || ' ' || u.last_name), u.avatar_file_id, i.declined FROM call_invites i JOIN users u ON u.id = i.user_id WHERE i.call_id = $1 ORDER BY u.first_name",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({
        "id": id, "chat_id": chat, "started_at": started_at, "ended_at": ended_at,
        "people": people.into_iter().map(|(id, n, a, d)| json!({ "id": id, "name": n, "avatar_file_id": a, "declined": d })).collect::<Vec<_>>(),
        "in_call": peers(&state, id).await?.into_iter().map(|(p, u, n)| json!({ "peer_id": p, "user_id": u, "name": n })).collect::<Vec<_>>(),
        "media": media,
        "can_record": media == "livekit" && crate::livekit::can_record(&state),
        "recording": recording_now(&state, id).await?,
        "host_id": host,
        "allow": { "audio": allow_audio, "video": allow_video, "screen": allow_screen },
    })))
}

#[derive(Deserialize)]
struct Controls {
    allow_audio: Option<bool>,
    allow_video: Option<bool>,
    allow_screen: Option<bool>,
    /// Turn everyone else's microphone off now (they may turn it back on if allowed).
    #[serde(default)]
    mute_all: bool,
}

/// The host decides what the others may use, and can mute everyone.
async fn controls(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<Controls>) -> AppResult<Json<Value>> {
    check(&state, &auth, id).await?;
    let host: Option<Uuid> = sqlx::query_scalar("SELECT started_by FROM calls WHERE id = $1").bind(id).fetch_one(&state.db).await?;
    if host != Some(auth.id) {
        return Err(AppError::bad("only whoever started the call controls it"));
    }
    sqlx::query(
        "UPDATE calls SET allow_audio = COALESCE($2, allow_audio), allow_video = COALESCE($3, allow_video), allow_screen = COALESCE($4, allow_screen) WHERE id = $1",
    )
    .bind(id)
    .bind(b.allow_audio)
    .bind(b.allow_video)
    .bind(b.allow_screen)
    .execute(&state.db)
    .await?;
    let (a, v, sc): (bool, bool, bool) = sqlx::query_as("SELECT allow_audio, allow_video, allow_screen FROM calls WHERE id = $1").bind(id).fetch_one(&state.db).await?;
    let people: Vec<Uuid> = peers(&state, id).await?.into_iter().map(|(_, u, _)| u).filter(|u| *u != auth.id).collect();
    crate::realtime::publish(&state, Some(people), "call.controls", json!({
        "call_id": id, "allow": { "audio": a, "video": v, "screen": sc }, "mute_all": b.mute_all || !a,
    }))
    .await;
    Ok(Json(json!({ "allow": { "audio": a, "video": v, "screen": sc } })))
}

#[derive(Deserialize)]
struct HistoryQuery {
    before: Option<DateTime<Utc>>,
    limit: Option<i64>,
    /// "true": only calls I missed.
    missed: Option<String>,
}

/// My calls, newest first: who, when, how long, and whether I was in it.
async fn history(State(state): State<AppState>, auth: AuthUser, axum::extract::Query(q): axum::extract::Query<HistoryQuery>) -> AppResult<Json<Value>> {
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    #[allow(clippy::type_complexity)]
    let rows: Vec<(Uuid, Option<Uuid>, Option<String>, Option<Uuid>, Option<String>, DateTime<Utc>, Option<DateTime<Utc>>, bool, Vec<String>, bool)> = sqlx::query_as(
        "SELECT c.id, c.chat_id,
            (SELECT CASE WHEN ch.kind = 'direct' THEN NULL ELSE ch.title END FROM im_chats ch WHERE ch.id = c.chat_id),
            c.started_by, (SELECT trim(first_name || ' ' || last_name) FROM users WHERE id = c.started_by),
            c.started_at, c.ended_at,
            EXISTS (SELECT 1 FROM call_peers p WHERE p.call_id = c.id AND p.user_id = $1),
            ARRAY(SELECT trim(u.first_name || ' ' || u.last_name) FROM call_invites i2 JOIN users u ON u.id = i2.user_id
                  WHERE i2.call_id = c.id AND i2.user_id <> $1 ORDER BY u.first_name),
            EXISTS (SELECT 1 FROM call_recordings r WHERE r.call_id = c.id)
         FROM calls c JOIN call_invites i ON i.call_id = c.id AND i.user_id = $1
         WHERE ($2::timestamptz IS NULL OR c.started_at < $2)
           AND (NOT $4 OR (c.started_by IS DISTINCT FROM $1 AND NOT EXISTS (SELECT 1 FROM call_peers p WHERE p.call_id = c.id AND p.user_id = $1)))
         ORDER BY c.started_at DESC LIMIT $3",
    )
    .bind(auth.id)
    .bind(q.before)
    .bind(limit)
    .bind(q.missed.as_deref() == Some("true"))
    .fetch_all(&state.db)
    .await?;
    let items: Vec<Value> = rows
        .into_iter()
        .map(|(id, chat, chat_title, by, by_name, start, end, joined, others, recorded)| {
            let outgoing = by == Some(auth.id);
            json!({
                "id": id, "chat_id": chat, "chat_title": chat_title, "started_by": by, "started_by_name": by_name,
                "started_at": start, "ended_at": end,
                "seconds": end.map(|e| (e - start).num_seconds()),
                "outgoing": outgoing, "joined": joined, "missed": !outgoing && !joined && end.is_some(),
                "people": others, "recorded": recorded,
            })
        })
        .collect();
    Ok(Json(json!({ "items": items })))
}

async fn join(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    check(&state, &auth, id).await?;
    drop_silent(&state, id).await?;
    let ended: Option<DateTime<Utc>> = sqlx::query_scalar("SELECT ended_at FROM calls WHERE id = $1").bind(id).fetch_one(&state.db).await?;
    if ended.is_some() {
        return Err(AppError::bad("this call has ended"));
    }
    if peers(&state, id).await?.len() >= 8 {
        return Err(AppError::bad("the call is full"));
    }
    let peer: Uuid = sqlx::query_scalar("INSERT INTO call_peers (call_id, user_id) VALUES ($1, $2) RETURNING peer_id").bind(id).bind(auth.id).fetch_one(&state.db).await?;
    sqlx::query("UPDATE call_invites SET declined = FALSE WHERE call_id = $1 AND user_id = $2").bind(id).bind(auth.id).execute(&state.db).await?;
    let all = peers(&state, id).await?;
    let me = name(&state, auth.id).await;
    let others: Vec<Uuid> = all.iter().filter(|(p, ..)| *p != peer).map(|(_, u, _)| *u).collect();
    crate::realtime::publish(&state, Some(others), "call.peer", json!({ "call_id": id, "event": "joined", "peer_id": peer, "user_id": auth.id, "name": me })).await;
    let media: String = sqlx::query_scalar("SELECT media FROM calls WHERE id = $1").bind(id).fetch_one(&state.db).await?;
    if media == "livekit" {
        let cfg = state.config.livekit.as_ref().ok_or_else(|| AppError::bad("this call needs the video server, which is no longer configured"))?;
        // The browser's identity in the room is its peer id; who it is travels as metadata.
        let token = crate::livekit::join_token(cfg, &crate::livekit::room_name(id), &peer.to_string(), &me, &json!({ "user_id": auth.id }));
        return Ok(Json(json!({ "peer_id": peer, "peers": [], "livekit": { "url": cfg.url, "token": token } })));
    }
    Ok(Json(json!({
        "peer_id": peer,
        // The newcomer offers connections to everyone already in.
        "peers": all.into_iter().filter(|(p, ..)| *p != peer).map(|(p, u, n)| json!({ "peer_id": p, "user_id": u, "name": n })).collect::<Vec<_>>(),
        "ice_servers": state.config.webrtc_ice_servers,
    })))
}

#[derive(Deserialize)]
struct SignalBody {
    from: Uuid,
    to: Uuid,
    /// offer | answer | ice
    kind: String,
    data: Value,
}

/// Passes a setup message from one browser in the call to another.
async fn signal(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<SignalBody>) -> AppResult<Json<Value>> {
    if !["offer", "answer", "ice"].contains(&b.kind.as_str()) {
        return Err(AppError::bad("kind must be offer, answer or ice"));
    }
    if b.data.to_string().len() > 64 * 1024 {
        return Err(AppError::bad("the message is too large"));
    }
    let all = peers(&state, id).await?;
    if !all.iter().any(|(p, u, _)| *p == b.from && *u == auth.id) {
        return Err(AppError::bad("you are not in this call"));
    }
    let to = all.iter().find(|(p, ..)| *p == b.to).map(|(_, u, _)| *u).ok_or_else(|| AppError::bad("that person has left the call"))?;
    crate::realtime::publish(&state, Some(vec![to]), "call.signal", json!({ "call_id": id, "from": b.from, "to": b.to, "kind": b.kind, "data": b.data })).await;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct PeerBody {
    peer_id: Uuid,
}

async fn heartbeat(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<PeerBody>) -> AppResult<Json<Value>> {
    let n = sqlx::query("UPDATE call_peers SET seen_at = now() WHERE peer_id = $1 AND call_id = $2 AND user_id = $3 AND left_at IS NULL")
        .bind(b.peer_id)
        .bind(id)
        .bind(auth.id)
        .execute(&state.db)
        .await?
        .rows_affected();
    if n == 0 {
        return Err(AppError::bad("you are no longer in this call"));
    }
    drop_silent(&state, id).await?;
    Ok(Json(json!({ "ok": true })))
}

/// Takes out browsers that stopped sending heartbeats (closed tab, lost connection).
async fn drop_silent(state: &AppState, call: Uuid) -> AppResult<()> {
    let gone: Vec<(Uuid, Uuid)> = sqlx::query_as(
        "UPDATE call_peers SET left_at = now() WHERE call_id = $1 AND left_at IS NULL AND seen_at < now() - make_interval(secs => $2) RETURNING peer_id, user_id",
    )
    .bind(call)
    .bind(SILENT_SECS as f64)
    .fetch_all(&state.db)
    .await?;
    for (peer, user) in gone {
        left(state, call, peer, user).await?;
    }
    Ok(())
}

async fn left(state: &AppState, call: Uuid, peer: Uuid, user: Uuid) -> AppResult<()> {
    let rest = peers(state, call).await?;
    crate::realtime::publish(state, Some(rest.iter().map(|(_, u, _)| *u).collect()), "call.peer", json!({ "call_id": call, "event": "left", "peer_id": peer, "user_id": user })).await;
    if rest.is_empty() {
        let row: Option<(Option<Uuid>, DateTime<Utc>)> = sqlx::query_as("UPDATE calls SET ended_at = now() WHERE id = $1 AND ended_at IS NULL RETURNING chat_id, started_at")
            .bind(call)
            .fetch_optional(&state.db)
            .await?;
        if let Some((chat, started)) = row {
            let mins = (Utc::now() - started).num_minutes();
            chat_note(state, chat, format!("Call ended · {}", if mins < 1 { "under a minute".to_string() } else { format!("{mins} min") })).await;
            crate::realtime::publish(state, Some(invited(state, call).await?), "call.ended", json!({ "call_id": call })).await;
            // The call is over: so is its recording.
            finish_recording(state, call, None).await;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Recording (calls through LiveKit)
// ---------------------------------------------------------------------------

pub const RECORDINGS_JOB: &str = "calls.recordings";

/// The recording going on now, if any.
async fn recording_now(state: &AppState, call: Uuid) -> AppResult<Option<Value>> {
    let row: Option<(Uuid, Option<Uuid>, DateTime<Utc>, Option<String>)> = sqlx::query_as(
        "SELECT r.id, r.started_by, r.started_at, trim(u.first_name || ' ' || u.last_name) FROM call_recordings r LEFT JOIN users u ON u.id = r.started_by
         WHERE r.call_id = $1 AND r.status = 'recording' ORDER BY r.started_at DESC LIMIT 1",
    )
    .bind(call)
    .fetch_optional(&state.db)
    .await?;
    Ok(row.map(|(id, by, at, n)| json!({ "id": id, "started_by": by, "started_by_name": n, "started_at": at })))
}

/// Anyone in the call may start recording; everyone sees that it is being recorded.
async fn start_recording(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let who = check(&state, &auth, id).await?;
    let (media, ended, chat): (String, Option<DateTime<Utc>>, Option<Uuid>) =
        sqlx::query_as("SELECT media, ended_at, chat_id FROM calls WHERE id = $1").bind(id).fetch_one(&state.db).await?;
    if ended.is_some() {
        return Err(AppError::bad("this call has ended"));
    }
    if media != "livekit" || !crate::livekit::can_record(&state) {
        return Err(AppError::bad("recording needs the video server (LiveKit) with recording set up"));
    }
    if !peers(&state, id).await?.iter().any(|(_, u, _)| *u == auth.id) {
        return Err(AppError::bad("join the call to record it"));
    }
    // Claim the call's one recording first, so presses at the same moment start only one.
    let rec = Uuid::new_v4();
    let file_name = format!("nerp-call-{id}-{rec}.mp4");
    sqlx::query("INSERT INTO call_recordings (id, call_id, file_name, started_by) VALUES ($1, $2, $3, $4)")
        .bind(rec)
        .bind(id)
        .bind(&file_name)
        .bind(auth.id)
        .execute(&state.db)
        .await
        .map_err(|e| if crate::error::is_unique_violation(&e) { AppError::Conflict("this call is already being recorded".into()) } else { e.into() })?;
    let egress = match crate::livekit::start_recording(&state, &crate::livekit::room_name(id), &file_name).await {
        Ok(e) => e,
        Err(e) => {
            sqlx::query("DELETE FROM call_recordings WHERE id = $1").bind(rec).execute(&state.db).await?;
            return Err(AppError::BadRequest(e));
        }
    };
    sqlx::query("UPDATE call_recordings SET egress_id = $2 WHERE id = $1").bind(rec).bind(&egress).execute(&state.db).await?;
    let me = name(&state, auth.id).await;
    chat_note(&state, chat, format!("{me} started recording the call")).await;
    let now = recording_now(&state, id).await?;
    crate::realtime::publish(&state, Some(who), "call.recording", json!({ "call_id": id, "recording": now })).await;
    Ok(Json(json!({ "recording": now })))
}

async fn stop_recording(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    check(&state, &auth, id).await?;
    if recording_now(&state, id).await?.is_none() {
        return Err(AppError::bad("the call is not being recorded"));
    }
    if !peers(&state, id).await?.iter().any(|(_, u, _)| *u == auth.id) {
        return Err(AppError::bad("join the call to stop the recording"));
    }
    finish_recording(&state, id, Some(auth.id)).await;
    Ok(Json(json!({ "recording": null })))
}

/// Stops the call's recording; Egress then finishes the file and the job collects it.
async fn finish_recording(state: &AppState, call: Uuid, by: Option<Uuid>) {
    let rows: Vec<(Uuid, Option<String>)> = sqlx::query_as(
        "UPDATE call_recordings SET status = 'processing', stopped_at = now() WHERE call_id = $1 AND status = 'recording' RETURNING id, egress_id",
    )
    .bind(call)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();
    if rows.is_empty() {
        return;
    }
    for (_, egress) in &rows {
        if let Some(e) = egress {
            if let Err(err) = crate::livekit::stop_recording(state, e).await {
                tracing::warn!(call = %call, error = %err, "stopping a recording failed");
            }
        }
    }
    let chat: Option<Uuid> = sqlx::query_scalar("SELECT chat_id FROM calls WHERE id = $1").bind(call).fetch_optional(&state.db).await.ok().flatten().flatten();
    let who = match by {
        Some(u) => name(state, u).await,
        None => String::new(),
    };
    chat_note(state, chat, if who.is_empty() { "Recording stopped; it will appear here when it is ready".into() } else { format!("{who} stopped the recording; it will appear here when it is ready") }).await;
    if let Ok(people) = invited(state, call).await {
        crate::realtime::publish(state, Some(people), "call.recording", json!({ "call_id": call, "recording": null })).await;
    }
}

/// Every 20 seconds: collects recordings Egress has finished (into NERP's files, posted in the
/// call's chat) and notes the ones that failed.
pub async fn run_recordings(state: &AppState) -> Result<()> {
    if state.config.livekit.is_none() {
        return Ok(());
    }
    type Row = (Uuid, Uuid, Option<String>, String, Option<Uuid>, String, DateTime<Utc>);
    let open: Vec<Row> = sqlx::query_as(
        "SELECT id, call_id, egress_id, file_name, started_by, status, started_at FROM call_recordings WHERE status IN ('recording', 'processing')",
    )
    .fetch_all(&state.db)
    .await?;
    for (rec, call, egress, file_name, by, status, started_at) in open {
        // Claimed but Egress never started it (NERP stopped in between).
        let Some(egress) = egress else {
            if Utc::now() - started_at > chrono::Duration::minutes(2) {
                fail(state, rec, call, "the recording did not start").await;
            }
            continue;
        };
        let info = match crate::livekit::recording_status(state, &egress).await {
            Ok(i) => i,
            Err(e) => {
                tracing::warn!(error = %e, "cannot ask the video server about a recording");
                continue;
            }
        };
        match info {
            Some((s, _, duration)) if s == "EGRESS_COMPLETE" || (s == "EGRESS_LIMIT_REACHED" && recording_file(state, &file_name).is_some()) => {
                if let Err(e) = collect(state, rec, call, &file_name, by, duration).await {
                    tracing::warn!(error = ?e, "collecting a recording failed");
                    fail(state, rec, call, &e.to_string()).await;
                }
            }
            Some((s, error, _)) if s == "EGRESS_FAILED" || s == "EGRESS_ABORTED" || s == "EGRESS_LIMIT_REACHED" => {
                fail(state, rec, call, &error.unwrap_or_else(|| s.trim_start_matches("EGRESS_").to_lowercase())).await;
            }
            // Egress ended it by itself (e.g. the room emptied): the call's chat hears it is coming.
            Some((s, ..)) if s == "EGRESS_ENDING" && status == "recording" => finish_recording(state, call, None).await,
            Some(_) => {}
            None if Utc::now() - started_at > chrono::Duration::hours(1) => fail(state, rec, call, "the video server lost the recording").await,
            None => {}
        }
    }
    Ok(())
}

fn recording_file(state: &AppState, file_name: &str) -> Option<std::path::PathBuf> {
    let dir = state.config.livekit.as_ref()?.recordings_dir.as_ref()?;
    let p = dir.join(file_name);
    std::fs::metadata(&p).ok().filter(|m| m.len() > 0).map(|_| p)
}

async fn collect(state: &AppState, rec: Uuid, call: Uuid, file_name: &str, by: Option<Uuid>, duration: Option<i32>) -> AppResult<()> {
    let path = recording_file(state, file_name).ok_or_else(|| AppError::bad("the recording file is not in the recordings folder"))?;
    let started: DateTime<Utc> = sqlx::query_scalar("SELECT started_at FROM call_recordings WHERE id = $1").bind(rec).fetch_one(&state.db).await?;
    let title = format!("Call recording {}.mp4", started.format("%Y-%m-%d %H-%M"));
    let file = crate::routes::files::store_path(state, by, &title, "video/mp4", &path).await?;
    let n = sqlx::query("UPDATE call_recordings SET status = 'ready', file_id = $2, duration_secs = $3 WHERE id = $1 AND status <> 'ready'")
        .bind(rec)
        .bind(file)
        .bind(duration)
        .execute(&state.db)
        .await?
        .rows_affected();
    if n == 0 {
        crate::routes::files::delete_stored(state, file).await;
        return Ok(());
    }
    let _ = tokio::fs::remove_file(&path).await;
    // Posted in the call's chat by whoever recorded it, so its members can watch it.
    let chat: Option<Uuid> = sqlx::query_scalar("SELECT chat_id FROM calls WHERE id = $1").bind(call).fetch_one(&state.db).await?;
    let length = duration.map(|d| format!(" ({} min {} s)", d / 60, d % 60)).unwrap_or_default();
    if let Some(c) = chat {
        crate::im::post_message(state, c, by, &format!("Recording of the call{length}"), &[file]).await?;
    }
    if let Some(u) = by {
        notify(state, &[u], NewNotification {
            kind: "call.recording",
            title: format!("Your call recording is ready{length}"),
            body: None,
            link: Some(match chat {
                Some(c) => format!("/messenger?chat={c}"),
                None => "/files".into(),
            }),
            actor: None,
        })
        .await;
    }
    Ok(())
}

async fn fail(state: &AppState, rec: Uuid, call: Uuid, error: &str) {
    let n = sqlx::query("UPDATE call_recordings SET status = 'failed', error = $2, stopped_at = COALESCE(stopped_at, now()) WHERE id = $1 AND status IN ('recording', 'processing')")
        .bind(rec)
        .bind(error)
        .execute(&state.db)
        .await
        .map(|r| r.rows_affected())
        .unwrap_or(0);
    if n == 1 {
        let chat: Option<Uuid> = sqlx::query_scalar("SELECT chat_id FROM calls WHERE id = $1").bind(call).fetch_optional(&state.db).await.ok().flatten().flatten();
        chat_note(state, chat, format!("The recording failed: {error}")).await;
        if let Ok(people) = invited(state, call).await {
            crate::realtime::publish(state, Some(people), "call.recording", json!({ "call_id": call, "recording": null })).await;
        }
    }
}

async fn leave(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<PeerBody>) -> AppResult<Json<Value>> {
    check(&state, &auth, id).await?;
    let n = sqlx::query("UPDATE call_peers SET left_at = now() WHERE peer_id = $1 AND call_id = $2 AND user_id = $3 AND left_at IS NULL")
        .bind(b.peer_id)
        .bind(id)
        .bind(auth.id)
        .execute(&state.db)
        .await?
        .rows_affected();
    if n == 1 {
        left(&state, id, b.peer_id, auth.id).await?;
    }
    Ok(Json(json!({ "ok": true })))
}

/// "Not now": the others see it; a call nobody joined ends when everyone has declined.
async fn decline(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let who = check(&state, &auth, id).await?;
    sqlx::query("UPDATE call_invites SET declined = TRUE WHERE call_id = $1 AND user_id = $2").bind(id).bind(auth.id).execute(&state.db).await?;
    let me = name(&state, auth.id).await;
    crate::realtime::publish(&state, Some(who.into_iter().filter(|u| *u != auth.id).collect()), "call.declined", json!({ "call_id": id, "user_id": auth.id, "name": me })).await;
    Ok(Json(json!({ "ok": true })))
}

/// Every minute: ends calls whose browsers all fell silent.
pub async fn run_cleanup(state: &AppState) -> Result<()> {
    let open: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM calls WHERE ended_at IS NULL").fetch_all(&state.db).await?;
    for id in open {
        drop_silent(state, id).await.map_err(|e| anyhow::anyhow!("{e:?}"))?;
        // Rang but nobody ever joined, an hour on.
        sqlx::query("UPDATE calls SET ended_at = now() WHERE id = $1 AND ended_at IS NULL AND started_at < now() - interval '1 hour'
                     AND NOT EXISTS (SELECT 1 FROM call_peers WHERE call_id = $1 AND left_at IS NULL)")
            .bind(id)
            .execute(&state.db)
            .await?;
    }
    Ok(())
}

pub const CLEANUP_JOB: &str = "calls.cleanup";
