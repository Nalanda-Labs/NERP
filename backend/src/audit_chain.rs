//! Tamper-evident audit trail.
//!
//! Audit records are written freely (src/audit.rs) and sealed into a hash chain every minute
//! by the `audit.seal` job, in the order they were written: each record's hash covers its
//! content and the previous record's hash, so changing, removing or reordering any record
//! breaks every hash after it. The database refuses changes and removals outright
//! (migration 0103); the chain catches what someone with direct database access does anyway.
//!
//! To catch a rewritten chain (every hash recomputed after a change), the head is also
//! checkpointed hourly with a signature keyed by DATA_ENCRYPTION_KEY, which never goes into
//! the database. Administrators can also note the head (position + hash) somewhere else and
//! check it later.
use axum::{extract::State, routing::{get, post}, Json, Router};
use chrono::{DateTime, Utc};
use hmac::{Hmac, KeyInit, Mac};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    audit,
    auth::{permissions as perm, AuthUser},
    error::{AppError, AppResult},
    auth::ClientIp,
    state::AppState,
};

pub const SEAL_JOB: &str = "audit.seal";
pub const VERIFY_JOB: &str = "audit.verify";
const LOCK: i64 = 0x6e65_7270_6175_6431; // "nerpaud1"
const BATCH: i64 = 5000;
const ANCHOR_EVERY_SECS: i64 = 3600;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/audit/chain", get(status))
        .route("/audit/chain/verify", post(verify_now))
        .route("/audit/chain/checkpoint", post(checkpoint_now))
}

#[derive(sqlx::FromRow)]
struct Rec {
    id: Uuid,
    chain_pos: Option<i64>,
    prev_hash: Option<Vec<u8>>,
    hash: Option<Vec<u8>>,
    actor: Option<Uuid>,
    action: String,
    entity_type: String,
    entity_id: Option<String>,
    details: String,
    ip: Option<String>,
    created_at: DateTime<Utc>,
}

const COLS: &str = "id, chain_pos, prev_hash, hash, actor, action, entity_type, entity_id, details::text AS details, ip, created_at";

fn field(h: &mut Sha256, v: Option<&[u8]>) {
    match v {
        Some(b) => {
            h.update((b.len() as u32).to_be_bytes());
            h.update(b);
        }
        None => h.update(u32::MAX.to_be_bytes()),
    }
}

/// The hash of a record at `pos` following `prev`.
fn record_hash(prev: &[u8], pos: i64, r: &Rec) -> Vec<u8> {
    let mut h = Sha256::new();
    h.update(b"nerp-audit-v1");
    h.update(prev);
    h.update(pos.to_be_bytes());
    h.update(r.id.as_bytes());
    field(&mut h, r.actor.as_ref().map(|u| &u.as_bytes()[..]));
    field(&mut h, Some(r.action.as_bytes()));
    field(&mut h, Some(r.entity_type.as_bytes()));
    field(&mut h, r.entity_id.as_deref().map(str::as_bytes));
    field(&mut h, Some(r.details.as_bytes()));
    field(&mut h, r.ip.as_deref().map(str::as_bytes));
    h.update(r.created_at.timestamp_micros().to_be_bytes());
    h.finalize().to_vec()
}

fn anchor_mac(state: &AppState, pos: i64, hash: &[u8], at: DateTime<Utc>) -> Vec<u8> {
    let mut k = <Hmac<Sha256> as KeyInit>::new_from_slice(&state.config.data_key).expect("any key length");
    k.update(b"nerp-audit-anchor-key");
    let key = k.finalize().into_bytes();
    let mut m = <Hmac<Sha256> as KeyInit>::new_from_slice(&key).expect("any key length");
    m.update(&pos.to_be_bytes());
    m.update(hash);
    m.update(&at.timestamp_micros().to_be_bytes());
    m.finalize().into_bytes().to_vec()
}

const GENESIS: [u8; 32] = [0; 32];

/// Seals every unsealed record into the chain. Returns how many were sealed.
pub async fn seal(state: &AppState) -> anyhow::Result<i64> {
    let mut total = 0;
    loop {
        let mut tx = state.db.begin().await?;
        let locked: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock($1)").bind(LOCK).fetch_one(&mut *tx).await?;
        if !locked {
            return Ok(total);
        }
        let head: Option<(i64, Vec<u8>)> = sqlx::query_as("SELECT chain_pos, hash FROM audit_log WHERE chain_pos IS NOT NULL ORDER BY chain_pos DESC LIMIT 1")
            .fetch_optional(&mut *tx)
            .await?;
        let (mut pos, mut prev) = head.unwrap_or((0, GENESIS.to_vec()));
        let rows: Vec<Rec> = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {COLS} FROM audit_log WHERE chain_pos IS NULL ORDER BY created_at, id LIMIT $1 FOR UPDATE")))
            .bind(BATCH)
            .fetch_all(&mut *tx)
            .await?;
        if rows.is_empty() {
            return Ok(total);
        }
        let (mut ids, mut poss, mut prevs, mut hashes) = (vec![], vec![], vec![], vec![]);
        for r in &rows {
            pos += 1;
            let h = record_hash(&prev, pos, r);
            ids.push(r.id);
            poss.push(pos);
            prevs.push(prev.clone());
            hashes.push(h.clone());
            prev = h;
        }
        sqlx::query(
            "UPDATE audit_log a SET chain_pos = v.pos, prev_hash = v.prev, hash = v.h
             FROM UNNEST($1::uuid[], $2::bigint[], $3::bytea[], $4::bytea[]) AS v(id, pos, prev, h) WHERE a.id = v.id",
        )
        .bind(&ids)
        .bind(&poss)
        .bind(&prevs)
        .bind(&hashes)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        total += rows.len() as i64;
        if (rows.len() as i64) < BATCH {
            return Ok(total);
        }
    }
}

/// Writes a signed checkpoint of the head if it moved and the last one is older than `every`
/// seconds (0 = whenever it moved).
async fn anchor(state: &AppState, every: i64) -> anyhow::Result<Option<i64>> {
    let head: Option<(i64, Vec<u8>)> = sqlx::query_as("SELECT chain_pos, hash FROM audit_log WHERE chain_pos IS NOT NULL ORDER BY chain_pos DESC LIMIT 1")
        .fetch_optional(&state.db)
        .await?;
    let Some((pos, hash)) = head else { return Ok(None) };
    let last: Option<(i64, DateTime<Utc>)> = sqlx::query_as("SELECT chain_pos, created_at FROM audit_anchors ORDER BY id DESC LIMIT 1").fetch_optional(&state.db).await?;
    if let Some((lpos, at)) = last {
        if lpos == pos || (Utc::now() - at).num_seconds() < every {
            return Ok(None);
        }
    }
    // Truncated to microseconds, as the database keeps it.
    let at = DateTime::<Utc>::from_timestamp_micros(Utc::now().timestamp_micros()).unwrap_or_else(Utc::now);
    let mac = anchor_mac(state, pos, &hash, at);
    sqlx::query("INSERT INTO audit_anchors (chain_pos, hash, mac, created_at) VALUES ($1, $2, $3, $4)")
        .bind(pos)
        .bind(&hash)
        .bind(&mac)
        .bind(at)
        .execute(&state.db)
        .await?;
    Ok(Some(pos))
}

pub async fn run_seal(state: &AppState) -> anyhow::Result<()> {
    seal(state).await?;
    anchor(state, ANCHOR_EVERY_SECS).await?;
    Ok(())
}

/// Daily: verifies the whole chain and tells administrators if it doesn't hold.
pub async fn run_verify(state: &AppState) -> anyhow::Result<()> {
    seal(state).await?;
    let r = verify(state, None).await?;
    if r["ok"] != json!(true) {
        let admins: Vec<Uuid> = sqlx::query_scalar(
            "SELECT DISTINCT ur.user_id FROM user_roles ur JOIN role_permissions rp ON rp.role_id = ur.role_id
             JOIN users u ON u.id = ur.user_id WHERE rp.permission IN ('*', $1) AND u.is_active",
        )
        .bind(perm::AUDIT_VIEW)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();
        crate::notify::notify(
            state,
            &admins,
            crate::notify::NewNotification {
                kind: "audit.tampered",
                title: "The audit trail doesn't verify".into(),
                body: Some(format!("{} — see Audit log → Integrity.", r["problems"][0]["text"].as_str().unwrap_or("a record was changed or removed"))),
                link: Some("/audit?tab=integrity".into()),
                actor: None,
            },
        )
        .await;
    }
    Ok(())
}

#[derive(Deserialize, Default)]
pub struct VerifyInput {
    /// A head noted earlier, to check it is still part of the chain.
    checkpoint: Option<Checkpoint>,
}

#[derive(Deserialize)]
struct Checkpoint {
    position: i64,
    hash: String,
}

/// Walks the whole chain and the checkpoints. Problems are listed (the first 20).
async fn verify(state: &AppState, cp: Option<&Checkpoint>) -> anyhow::Result<Value> {
    let started = std::time::Instant::now();
    let mut tx = state.db.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY").execute(&mut *tx).await?;
    let mut problems: Vec<Value> = vec![];
    let mut problem = |pos: i64, kind: &str, text: String| {
        if problems.len() < 20 {
            problems.push(json!({ "position": pos, "kind": kind, "text": text }));
        }
    };
    let mut prev = GENESIS.to_vec();
    let mut expect = 1i64;
    let mut hashes: std::collections::HashMap<i64, Vec<u8>> = Default::default();
    let anchors: Vec<(i64, i64, Vec<u8>, Vec<u8>, DateTime<Utc>)> = sqlx::query_as("SELECT id, chain_pos, hash, mac, created_at FROM audit_anchors ORDER BY id").fetch_all(&mut *tx).await?;
    let wanted: std::collections::HashSet<i64> = anchors.iter().map(|a| a.1).chain(cp.map(|c| c.position)).collect();
    loop {
        let rows: Vec<Rec> = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {COLS} FROM audit_log WHERE chain_pos >= $1 ORDER BY chain_pos LIMIT $2")))
            .bind(expect)
            .bind(BATCH)
            .fetch_all(&mut *tx)
            .await?;
        if rows.is_empty() {
            break;
        }
        for r in &rows {
            let pos = r.chain_pos.unwrap_or_default();
            if pos != expect {
                problem(expect, "missing", format!("Records {expect}–{} are missing", pos - 1));
                // Carry on from what is there, against its own previous hash.
                prev = r.prev_hash.clone().unwrap_or_default();
            }
            if r.prev_hash.as_deref() != Some(&prev[..]) {
                problem(pos, "order", format!("Record {pos} doesn't follow the one before it"));
            }
            let h = record_hash(r.prev_hash.as_deref().unwrap_or(&prev), pos, r);
            if r.hash.as_deref() != Some(&h[..]) {
                problem(pos, "changed", format!("Record {pos} ({}, {}) was changed after it was written", r.action, r.created_at.format("%Y-%m-%d %H:%M UTC")));
            }
            if wanted.contains(&pos) {
                hashes.insert(pos, r.hash.clone().unwrap_or_default());
            }
            prev = r.hash.clone().unwrap_or_default();
            expect = pos + 1;
        }
    }
    let head = expect - 1;
    for (id, pos, hash, mac, at) in &anchors {
        if anchor_mac(state, *pos, hash, *at) != *mac {
            problem(*pos, "checkpoint", format!("Checkpoint {id} isn't signed by this server"));
        } else if *pos > head {
            problem(*pos, "removed", format!("Records after {head} are gone (a checkpoint of {} saw record {pos})", at.format("%Y-%m-%d %H:%M UTC")));
        } else if hashes.get(pos) != Some(hash) {
            problem(*pos, "rewritten", format!("The chain at record {pos} differs from the checkpoint of {} — it was rewritten", at.format("%Y-%m-%d %H:%M UTC")));
        }
    }
    let mut checkpoint = Value::Null;
    if let Some(c) = cp {
        let ok = hashes.get(&c.position).map(hex::encode).as_deref() == Some(c.hash.trim().to_lowercase().as_str());
        if !ok {
            problem(c.position, "noted", format!("The noted head (record {}) doesn't match the chain", c.position));
        }
        checkpoint = json!({ "position": c.position, "ok": ok });
    }
    let unsealed: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_log WHERE chain_pos IS NULL").fetch_one(&mut *tx).await?;
    tx.commit().await?;
    let result = json!({
        "ok": problems.is_empty(),
        "records": head,
        "head": { "position": head, "hash": hex::encode(&prev) },
        "checkpoints": anchors.len(),
        "last_checkpoint_at": anchors.last().map(|a| a.4),
        "unsealed": unsealed,
        "checkpoint": checkpoint,
        "problems": problems,
        "checked_at": Utc::now(),
        "took_ms": started.elapsed().as_millis() as u64,
    });
    sqlx::query("INSERT INTO settings (key, value) VALUES ('audit_verify_last', $1) ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value")
        .bind(&result)
        .execute(&state.db)
        .await?;
    Ok(result)
}

async fn status(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    auth.require(perm::AUDIT_VIEW)?;
    let head: Option<(i64, Vec<u8>, DateTime<Utc>)> = sqlx::query_as("SELECT chain_pos, hash, created_at FROM audit_log WHERE chain_pos IS NOT NULL ORDER BY chain_pos DESC LIMIT 1")
        .fetch_optional(&state.db)
        .await?;
    let unsealed: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_log WHERE chain_pos IS NULL").fetch_one(&state.db).await?;
    let last_cp: Option<DateTime<Utc>> = sqlx::query_scalar("SELECT max(created_at) FROM audit_anchors").fetch_one(&state.db).await?;
    let last: Option<Value> = sqlx::query_scalar("SELECT value FROM settings WHERE key = 'audit_verify_last'").fetch_optional(&state.db).await?;
    Ok(Json(json!({
        "head": head.map(|(p, h, at)| json!({ "position": p, "hash": hex::encode(h), "at": at })),
        "unsealed": unsealed,
        "last_checkpoint_at": last_cp,
        "last_verify": last,
    })))
}

async fn verify_now(State(state): State<AppState>, auth: AuthUser, ClientIp(ip): ClientIp, body: Option<Json<VerifyInput>>) -> AppResult<Json<Value>> {
    auth.require(perm::AUDIT_VIEW)?;
    let b = body.map(|b| b.0).unwrap_or_default();
    seal(&state).await.map_err(AppError::Internal)?;
    let r = verify(&state, b.checkpoint.as_ref()).await.map_err(AppError::Internal)?;
    audit::record(&state.db, Some(auth.id), "audit.verify", "audit", None, json!({ "ok": r["ok"], "records": r["records"] }), ip.as_deref()).await;
    Ok(Json(r))
}

/// Seals what is waiting and checkpoints the head now.
async fn checkpoint_now(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    auth.require(perm::AUDIT_VIEW)?;
    seal(&state).await.map_err(AppError::Internal)?;
    let pos = anchor(&state, 0).await.map_err(AppError::Internal)?;
    let head: Option<(i64, Vec<u8>)> = sqlx::query_as("SELECT chain_pos, hash FROM audit_log WHERE chain_pos IS NOT NULL ORDER BY chain_pos DESC LIMIT 1")
        .fetch_optional(&state.db)
        .await?;
    Ok(Json(json!({ "new": pos.is_some(), "head": head.map(|(p, h)| json!({ "position": p, "hash": hex::encode(h) })) })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(details: &str) -> Rec {
        Rec {
            id: Uuid::nil(),
            chain_pos: None,
            prev_hash: None,
            hash: None,
            actor: None,
            action: "a".into(),
            entity_type: "b".into(),
            entity_id: None,
            details: details.into(),
            ip: None,
            created_at: DateTime::<Utc>::from_timestamp(0, 0).unwrap(),
        }
    }

    #[test]
    fn hash_depends_on_everything() {
        let a = record_hash(&GENESIS, 1, &rec("{}"));
        assert_ne!(a, record_hash(&GENESIS, 2, &rec("{}")));
        assert_ne!(a, record_hash(&[1; 32], 1, &rec("{}")));
        assert_ne!(a, record_hash(&GENESIS, 1, &rec("{\"x\": 1}")));
        let mut r = rec("{}");
        r.entity_id = Some(String::new());
        assert_ne!(a, record_hash(&GENESIS, 1, &r), "NULL and empty differ");
    }
}
