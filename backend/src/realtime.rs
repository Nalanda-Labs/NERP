//! Real-time push to browsers.
//!
//! Every instance keeps the WebSocket connections of its own clients. Events are
//! published to one Redis channel; every instance subscribes and delivers each event to
//! whichever of its local connections it targets. So an event raised on instance A
//! reaches a user connected to instance B.
//!
//! Browsers cannot set an Authorization header on a WebSocket, so the client first
//! trades its access token for a single-use ticket (valid 30s) and connects with that.

use axum::{
    extract::{
        ws::{Message, WebSocket},
        Query, State, WebSocketUpgrade,
    },
    response::Response,
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use futures_util::{SinkExt, StreamExt};
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        RwLock,
    },
    time::Duration,
};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::{
    auth::AuthUser,
    error::{AppError, AppResult},
    state::AppState,
};

const CHANNEL: &str = "nerp:events";
const TICKET_TTL_SECS: u64 = 30;

/// An event as it travels through Redis.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Envelope {
    /// Recipients; `None` means everyone connected.
    pub users: Option<Vec<Uuid>>,
    /// Restrict delivery to one session (used to close a revoked session's sockets).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<Uuid>,
    #[serde(rename = "type")]
    pub kind: String,
    pub data: Value,
}

enum Outgoing {
    Text(String),
    Close,
}

struct Conn {
    session: Uuid,
    tx: mpsc::UnboundedSender<Outgoing>,
}

#[derive(Default)]
pub struct Hub {
    conns: RwLock<HashMap<Uuid, HashMap<u64, Conn>>>,
    next_id: AtomicU64,
}

impl Hub {
    fn add(&self, user: Uuid, session: Uuid, tx: mpsc::UnboundedSender<Outgoing>) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.conns.write().unwrap().entry(user).or_default().insert(id, Conn { session, tx });
        id
    }

    fn remove(&self, user: Uuid, id: u64) {
        let mut conns = self.conns.write().unwrap();
        if let Some(map) = conns.get_mut(&user) {
            map.remove(&id);
            if map.is_empty() {
                conns.remove(&user);
            }
        }
    }

    pub fn connection_count(&self) -> usize {
        self.conns.read().unwrap().values().map(HashMap::len).sum()
    }

    /// Delivers an event to the matching local connections.
    fn deliver(&self, env: &Envelope) {
        let text = json!({ "type": env.kind, "data": env.data }).to_string();
        let close = env.kind == "session.revoked";
        let conns = self.conns.read().unwrap();
        let mut send = |c: &Conn| {
            if env.session.is_some_and(|s| s != c.session) {
                return;
            }
            let _ = c.tx.send(Outgoing::Text(text.clone()));
            if close {
                let _ = c.tx.send(Outgoing::Close);
            }
        };
        match &env.users {
            Some(users) => {
                for u in users {
                    conns.get(u).into_iter().flat_map(HashMap::values).for_each(&mut send);
                }
            }
            None => conns.values().flat_map(HashMap::values).for_each(&mut send),
        }
    }
}

/// Publishes an event to every instance. Failures are logged, never propagated:
/// real-time delivery is best effort and the data is always in the database.
pub async fn publish(state: &AppState, users: Option<Vec<Uuid>>, kind: &str, data: Value) {
    publish_envelope(state, Envelope { users, session: None, kind: kind.into(), data }).await
}

pub async fn publish_envelope(state: &AppState, env: Envelope) {
    let mut redis = state.redis.clone();
    let payload = serde_json::to_string(&env).unwrap();
    if let Err(e) = redis.publish::<_, _, ()>(CHANNEL, payload).await {
        tracing::warn!(error = ?e, kind = env.kind, "could not publish real-time event");
    }
}

/// Subscribes to the event channel and feeds the local hub. Reconnects on failure.
pub fn start_subscriber(state: AppState, client: redis::Client) {
    tokio::spawn(async move {
        loop {
            match client.get_async_pubsub().await {
                Ok(mut pubsub) => {
                    if let Err(e) = pubsub.subscribe(CHANNEL).await {
                        tracing::warn!(error = ?e, "real-time subscribe failed");
                    } else {
                        tracing::info!("real-time event subscriber connected");
                        let mut stream = pubsub.on_message();
                        while let Some(msg) = stream.next().await {
                            let Ok(payload) = msg.get_payload::<String>() else { continue };
                            match serde_json::from_str::<Envelope>(&payload) {
                                Ok(env) => state.hub.deliver(&env),
                                Err(e) => tracing::warn!(error = ?e, "malformed real-time event"),
                            }
                        }
                        tracing::warn!("real-time event subscriber disconnected");
                    }
                }
                Err(e) => tracing::warn!(error = ?e, "real-time subscriber cannot reach Redis"),
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    });
}

// ---------------------------------------------------------------------------
// HTTP / WebSocket endpoints
// ---------------------------------------------------------------------------

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/realtime/ticket", post(ticket))
        .route("/realtime/ws", get(connect))
}

#[derive(Serialize, Deserialize)]
struct Ticket {
    user_id: Uuid,
    session_id: Uuid,
}

async fn ticket(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    let bytes: [u8; 24] = rand::random();
    let ticket = URL_SAFE_NO_PAD.encode(bytes);
    let mut redis = state.redis.clone();
    let _: () = redis
        .set_ex(
            format!("realtime:ticket:{ticket}"),
            serde_json::to_string(&Ticket { user_id: auth.id, session_id: auth.session_id }).unwrap(),
            TICKET_TTL_SECS,
        )
        .await?;
    Ok(Json(json!({ "ticket": ticket })))
}

#[derive(Deserialize)]
struct ConnectQuery {
    ticket: String,
}

async fn connect(
    State(state): State<AppState>,
    Query(q): Query<ConnectQuery>,
    ws: WebSocketUpgrade,
) -> AppResult<Response> {
    let mut redis = state.redis.clone();
    let raw: Option<String> = redis.get_del(format!("realtime:ticket:{}", q.ticket)).await?;
    let ticket: Ticket = raw
        .and_then(|r| serde_json::from_str(&r).ok())
        .ok_or(AppError::Unauthorized)?;
    Ok(ws.on_upgrade(move |socket| serve(state, socket, ticket)))
}

async fn serve(state: AppState, socket: WebSocket, ticket: Ticket) {
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<Outgoing>();
    let conn_id = state.hub.add(ticket.user_id, ticket.session_id, tx);
    let _ = sink
        .send(Message::Text(json!({ "type": "hello", "data": {} }).to_string().into()))
        .await;

    let mut ping = tokio::time::interval(Duration::from_secs(25));
    ping.tick().await;
    loop {
        tokio::select! {
            out = rx.recv() => match out {
                Some(Outgoing::Text(t)) => {
                    if sink.send(Message::Text(t.into())).await.is_err() { break; }
                }
                Some(Outgoing::Close) | None => {
                    let _ = sink.send(Message::Close(None)).await;
                    break;
                }
            },
            incoming = stream.next() => match incoming {
                // Clients only listen; anything but a close or error is ignored.
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                Some(Ok(_)) => {}
            },
            _ = ping.tick() => {
                if sink.send(Message::Ping(Vec::new().into())).await.is_err() { break; }
            }
        }
    }
    state.hub.remove(ticket.user_id, conn_id);
}
