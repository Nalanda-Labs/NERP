//! Everyone's mailboxes, for people with "Manage everyone's mailboxes": the list of every
//! connected mailbox with its state (owner, sharing, last sync, last error); pausing,
//! syncing and disconnecting them; and connecting mailboxes for many employees at once (one
//! server, each employee's address and password), each checked before it is kept.

use axum::{
    extract::{Path, State},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use super::{check, load_mailbox, queue_sync, settings, MailboxBody};
use crate::{
    auth::{permissions as perm, AuthUser},
    crypto,
    error::{AppError, AppResult},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/admin/mailboxes", get(list))
        .route("/admin/mailboxes/bulk", post(bulk))
        .route("/admin/mailboxes/{id}", axum::routing::patch(update).delete(remove))
        .route("/admin/mailboxes/{id}/sync", post(sync))
}

fn require(auth: &AuthUser) -> AppResult<()> {
    if auth.can(perm::MAIL_ADMIN) { Ok(()) } else { Err(AppError::Forbidden) }
}

async fn list(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    require(&auth)?;
    let rows: Vec<(Uuid, Uuid, String, String, String, String, bool, Option<DateTime<Utc>>, Option<String>, i32, i32, i32, i64, DateTime<Utc>)> = sqlx::query_as(
        "SELECT m.id, m.owner_id, trim(u.first_name || ' ' || u.last_name), u.email, m.email, m.auth_kind, m.sync_enabled, m.last_sync_at, m.last_error,
                cardinality(m.shared_with), cardinality(m.shared_roles), cardinality(m.shared_departments),
                (SELECT COUNT(*) FROM crm_emails e WHERE e.mailbox_id = m.id), m.created_at
         FROM crm_mailboxes m JOIN users u ON u.id = m.owner_id ORDER BY lower(u.last_name), lower(m.email)",
    )
    .fetch_all(&state.db)
    .await?;
    let items: Vec<Value> = rows
        .into_iter()
        .map(|(id, owner, name, owner_email, email, kind, on, last, err, sw, sr, sd, emails, at)| {
            let status = if !on { "paused" } else if err.is_some() { "error" } else if last.is_none() { "waiting" } else { "ok" };
            json!({
                "id": id, "owner": { "id": owner, "name": name, "email": owner_email }, "email": email, "auth_kind": kind,
                "sync_enabled": on, "last_sync_at": last, "last_error": err, "status": status,
                "shared": { "people": sw, "roles": sr, "departments": sd }, "emails": emails, "created_at": at,
            })
        })
        .collect();
    let without: Vec<(Uuid, String, String)> = sqlx::query_as(
        "SELECT u.id, trim(u.first_name || ' ' || u.last_name), u.email FROM users u
         WHERE u.is_active AND NOT u.is_guest AND NOT u.is_system AND NOT EXISTS (SELECT 1 FROM crm_mailboxes m WHERE m.owner_id = u.id)
         ORDER BY lower(u.last_name) LIMIT 2000",
    )
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({
        "items": items,
        "without_mailbox": without.into_iter().map(|(id, n, e)| json!({ "id": id, "name": n, "email": e })).collect::<Vec<_>>(),
    })))
}

#[derive(Deserialize)]
struct Server {
    imap_host: String,
    imap_port: Option<i32>,
    imap_security: Option<String>,
    smtp_host: String,
    smtp_port: Option<i32>,
    smtp_security: Option<String>,
    smtp_auth: Option<bool>,
    #[serde(default)]
    allow_invalid_certs: bool,
}

#[derive(Deserialize)]
struct Account {
    /// The employee (or their sign-in email).
    user_id: Option<Uuid>,
    user_email: Option<String>,
    /// The mailbox's address (default: the employee's own).
    email: Option<String>,
    username: Option<String>,
    password: String,
}

#[derive(Deserialize)]
struct Bulk {
    server: Server,
    accounts: Vec<Account>,
    sync_days: Option<i32>,
    #[serde(default)]
    create_leads: bool,
}

/// Connects mailboxes for employees: each is checked (signing in to IMAP and SMTP) and kept
/// for its employee, who is told. Accounts that fail are reported; the rest go on.
async fn bulk(State(state): State<AppState>, auth: AuthUser, Json(b): Json<Bulk>) -> AppResult<Json<Value>> {
    require(&auth)?;
    if b.accounts.is_empty() || b.accounts.len() > 500 {
        return Err(AppError::bad("connect between 1 and 500 mailboxes at a time"));
    }
    let mut results = Vec::new();
    let (mut connected, mut failed) = (0, 0);
    for a in b.accounts {
        let who: Option<(Uuid, String)> = match (a.user_id, a.user_email.as_deref()) {
            (Some(u), _) => sqlx::query_as("SELECT id, email FROM users WHERE id = $1 AND is_active AND NOT is_guest").bind(u).fetch_optional(&state.db).await?,
            (None, Some(e)) => sqlx::query_as("SELECT id, email FROM users WHERE email = $1 AND is_active AND NOT is_guest").bind(e.trim().to_lowercase()).fetch_optional(&state.db).await?,
            _ => None,
        };
        let label = a.email.clone().or(a.user_email.clone()).unwrap_or_default();
        let Some((user, user_email)) = who else {
            failed += 1;
            results.push(json!({ "email": label, "ok": false, "error": "no such employee" }));
            continue;
        };
        let email = a.email.clone().unwrap_or(user_email);
        let body = MailboxBody {
            email: Some(email.clone()),
            display_name: None,
            imap_host: Some(b.server.imap_host.clone()),
            imap_port: b.server.imap_port,
            imap_security: b.server.imap_security.clone(),
            smtp_host: Some(b.server.smtp_host.clone()),
            smtp_port: b.server.smtp_port,
            smtp_security: b.server.smtp_security.clone(),
            smtp_auth: b.server.smtp_auth,
            username: a.username.clone(),
            password: Some(a.password.clone()),
            allow_invalid_certs: Some(b.server.allow_invalid_certs),
            shared_with: None,
            shared_roles: None,
            shared_departments: None,
            create_leads: Some(b.create_leads),
            sync_days: b.sync_days,
            sync_enabled: Some(true),
        };
        let outcome: Result<Uuid, String> = async {
            let s = settings(body, None).map_err(|e| e.to_string())?;
            let taken: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM crm_mailboxes WHERE owner_id = $1 AND lower(email) = $2)").bind(user).bind(&s.email).fetch_one(&state.db).await.map_err(|e| e.to_string())?;
            if taken {
                return Err("already connected".into());
            }
            let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM crm_mailboxes WHERE owner_id = $1").bind(user).fetch_one(&state.db).await.map_err(|e| e.to_string())?;
            if n >= 10 {
                return Err("the employee already has 10 mailboxes".into());
            }
            let r = check(&state, &s).await;
            if r.0.is_err() || r.1.is_err() {
                return Err(format!("could not connect: {}", [r.0.err().map(|e| format!("IMAP {e}")), r.1.err()].into_iter().flatten().collect::<Vec<_>>().join("; ")));
            }
            let id: Uuid = sqlx::query_scalar(
                "INSERT INTO crm_mailboxes (owner_id, email, display_name, imap_host, imap_port, imap_security, smtp_host, smtp_port, smtp_security, smtp_auth,
                     username, secret, allow_invalid_certs, create_leads, sync_days, sync_enabled)
                 VALUES ($1, $2, '', $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, TRUE) RETURNING id",
            )
            .bind(user)
            .bind(&s.email)
            .bind(&s.imap_host)
            .bind(s.imap_port)
            .bind(&s.imap_security)
            .bind(&s.smtp_host)
            .bind(s.smtp_port)
            .bind(&s.smtp_security)
            .bind(s.smtp_auth)
            .bind(&s.username)
            .bind(crypto::seal(&state.config.data_key, &s.password))
            .bind(s.allow_invalid_certs)
            .bind(s.create_leads)
            .bind(s.sync_days)
            .fetch_one(&state.db)
            .await
            .map_err(|e| e.to_string())?;
            Ok(id)
        }
        .await;
        match outcome {
            Ok(id) => {
                connected += 1;
                crate::audit::record(&state.db, Some(auth.id), "crm.mailbox.connect", "crm_mailbox", Some(id.to_string()), json!({ "email": email, "for": user, "bulk": true }), None).await;
                let _ = queue_sync(&state, id).await;
                crate::notify::notify(
                    &state,
                    &[user],
                    crate::notify::NewNotification { kind: "crm.mailbox", title: format!("Your mailbox {email} was connected to NERP"), body: Some("Its email now files itself on CRM records; you can change its settings or disconnect it in Profile → Mailboxes.".into()), link: Some("/crm/mailboxes".into()), actor: Some(auth.id) },
                )
                .await;
                results.push(json!({ "email": email, "user_id": user, "ok": true, "id": id }));
            }
            Err(e) => {
                failed += 1;
                results.push(json!({ "email": email, "user_id": user, "ok": false, "error": e }));
            }
        }
    }
    Ok(Json(json!({ "connected": connected, "failed": failed, "results": results })))
}

#[derive(Deserialize)]
struct Update {
    sync_enabled: Option<bool>,
}

async fn update(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<Update>) -> AppResult<Json<Value>> {
    require(&auth)?;
    load_mailbox(&state, id).await?;
    if let Some(on) = b.sync_enabled {
        sqlx::query("UPDATE crm_mailboxes SET sync_enabled = $2, updated_at = now() WHERE id = $1").bind(id).bind(on).execute(&state.db).await?;
        crate::audit::record(&state.db, Some(auth.id), if on { "crm.mailbox.resume" } else { "crm.mailbox.pause" }, "crm_mailbox", Some(id.to_string()), json!({}), None).await;
    }
    Ok(Json(json!({ "ok": true })))
}

async fn sync(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    require(&auth)?;
    load_mailbox(&state, id).await?;
    Ok(Json(json!({ "queued": queue_sync(&state, id).await? })))
}

/// Disconnects a mailbox (filed emails stay on their records); its owner is told.
async fn remove(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    require(&auth)?;
    let m = load_mailbox(&state, id).await?;
    sqlx::query("DELETE FROM crm_mailboxes WHERE id = $1").bind(id).execute(&state.db).await?;
    crate::audit::record(&state.db, Some(auth.id), "crm.mailbox.disconnect", "crm_mailbox", Some(id.to_string()), json!({ "email": m.email, "by_admin": true }), None).await;
    if m.owner_id != auth.id {
        crate::notify::notify(
            &state,
            &[m.owner_id],
            crate::notify::NewNotification { kind: "crm.mailbox", title: format!("Your mailbox {} was disconnected from NERP", m.email), body: None, link: Some("/crm/mailboxes".into()), actor: Some(auth.id) },
        )
        .await;
    }
    Ok(Json(json!({ "ok": true })))
}
