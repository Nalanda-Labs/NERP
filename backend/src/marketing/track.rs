//! Public endpoints reached from campaign emails (no sign-in; the recipient's token is the
//! key): the open pixel, click redirects, and unsubscribing (the page, and one-click
//! unsubscribe from mail clients, RFC 8058).

use axum::{
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Redirect, Response},
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use super::send::link_sig;
use crate::{crm::timeline, error::{AppError, AppResult}, state::AppState};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/public/m/o/{token}", get(open))
        .route("/public/m/c/{token}", get(click))
        .route("/public/m/u/{token}", get(unsubscribe_info).post(unsubscribe))
}

/// A transparent 1×1 GIF.
const PIXEL: &[u8] = &[
    0x47, 0x49, 0x46, 0x38, 0x39, 0x61, 0x01, 0x00, 0x01, 0x00, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff, 0x21, 0xf9, 0x04, 0x01, 0x00, 0x00, 0x00, 0x00, 0x2c,
    0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x02, 0x02, 0x44, 0x01, 0x00, 0x3b,
];

async fn open(State(state): State<AppState>, Path(token): Path<String>) -> Response {
    let _ = sqlx::query("UPDATE mkt_recipients SET open_count = open_count + 1, opened_at = COALESCE(opened_at, now()) WHERE token = $1 AND status = 'sent'")
        .bind(&token)
        .execute(&state.db)
        .await;
    ([(header::CONTENT_TYPE, "image/gif"), (header::CACHE_CONTROL, "no-store, max-age=0")], PIXEL).into_response()
}

#[derive(Deserialize)]
struct ClickQuery {
    u: String,
    s: String,
}

/// Counts the click (a click also means the email was opened) and goes on to the link.
async fn click(State(state): State<AppState>, Path(token): Path<String>, Query(q): Query<ClickQuery>) -> Response {
    if link_sig(&state, &token, &q.u) != q.s || crate::outbound::check_url(&q.u).is_err() {
        return (StatusCode::BAD_REQUEST, "This link is not valid.").into_response();
    }
    let rid: Option<Uuid> = sqlx::query_scalar(
        "UPDATE mkt_recipients SET click_count = click_count + 1, clicked_at = COALESCE(clicked_at, now()), opened_at = COALESCE(opened_at, now()), open_count = GREATEST(open_count, 1)
         WHERE token = $1 AND status = 'sent' RETURNING id",
    )
    .bind(&token)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten();
    if let Some(rid) = rid {
        let _ = sqlx::query("INSERT INTO mkt_clicks (recipient_id, url) VALUES ($1, $2)").bind(rid).bind(&q.u).execute(&state.db).await;
    }
    Redirect::to(&q.u).into_response()
}

async fn recipient(state: &AppState, token: &str) -> AppResult<(Uuid, Option<Uuid>, String, Uuid, String, Option<chrono::DateTime<chrono::Utc>>)> {
    sqlx::query_as(
        "SELECT r.id, r.item_id, r.email, c.id, c.name, r.unsubscribed_at FROM mkt_recipients r JOIN mkt_campaigns c ON c.id = r.campaign_id WHERE r.token = $1",
    )
    .bind(token)
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::NotFound("link"))
}

/// "j***@example.com": enough for people to recognise their address, not to harvest it.
fn mask(email: &str) -> String {
    match email.split_once('@') {
        Some((user, domain)) => format!("{}***@{domain}", user.chars().next().unwrap_or('*')),
        None => "***".into(),
    }
}

async fn unsubscribe_info(State(state): State<AppState>, Path(token): Path<String>) -> AppResult<Json<Value>> {
    let (_, _, email, _, _, at) = recipient(&state, &token).await?;
    let out: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM mkt_optouts WHERE email = $1)").bind(&email).fetch_one(&state.db).await?;
    let company: Option<Value> = sqlx::query_scalar("SELECT value FROM settings WHERE key = 'company_name'").fetch_optional(&state.db).await?;
    Ok(Json(json!({ "email": mask(&email), "company": company, "unsubscribed": out || at.is_some() })))
}

/// Opts the address out of all campaigns. Works for the page's button and for mail
/// clients' one-click POST (any body).
async fn unsubscribe(State(state): State<AppState>, Path(token): Path<String>) -> AppResult<Json<Value>> {
    let (rid, item, email, campaign, name, at) = recipient(&state, &token).await?;
    sqlx::query("INSERT INTO mkt_optouts (email, campaign_id, source) VALUES ($1, $2, 'unsubscribed') ON CONFLICT DO NOTHING")
        .bind(&email)
        .bind(campaign)
        .execute(&state.db)
        .await?;
    if at.is_none() {
        sqlx::query("UPDATE mkt_recipients SET unsubscribed_at = now() WHERE id = $1").bind(rid).execute(&state.db).await?;
        if let Some(item) = item {
            let _ = timeline(&state.db, item, "campaign", None, None, json!({ "campaign_id": campaign, "name": name, "event": "unsubscribed" })).await;
        }
    }
    Ok(Json(json!({ "ok": true })))
}

#[cfg(test)]
mod tests {
    #[test]
    fn masking() {
        assert_eq!(super::mask("jane@example.com"), "j***@example.com");
        assert_eq!(super::mask("nope"), "***");
    }
}
