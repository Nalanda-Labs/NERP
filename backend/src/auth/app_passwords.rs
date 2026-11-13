//! App passwords: for apps that can't sign in through the browser (calendar apps over
//! CalDAV). Each is random, shown once, kept only as a hash, and can be revoked on its own.
//! They work only for those apps, never for signing in to NERP itself, so two-step sign-in
//! still guards the account.

use axum::{
    extract::{Path, State},
    routing::{delete, get},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use super::{tokens::hash_token, AuthUser, ClientIp};
use crate::{
    audit,
    error::{AppError, AppResult},
    routes::required,
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/auth/app-passwords", get(list).post(create))
        .route("/auth/app-passwords/{id}", delete(revoke))
}

/// Sixteen lower-case letters in groups of four ("abcd-efgh-ijkl-mnop"): easy to type on a
/// phone, 75 bits of chance.
fn new_password() -> String {
    const LETTERS: &[u8] = b"abcdefghijklmnopqrstuvwxyz";
    let mut buf = [0u8; 16];
    rand::fill(&mut buf[..]);
    let letters: Vec<char> = buf.iter().map(|b| LETTERS[(*b as usize) % 26] as char).collect();
    letters.chunks(4).map(|c| c.iter().collect::<String>()).collect::<Vec<_>>().join("-")
}

/// Dashes and spaces don't matter when typing it.
fn normalise(p: &str) -> String {
    p.chars().filter(|c| !c.is_whitespace() && *c != '-').collect::<String>().to_lowercase()
}

/// The active user an email and app password belong to.
pub async fn verify(state: &AppState, email: &str, password: &str, ip: Option<&str>) -> AppResult<Option<Uuid>> {
    let hash = hash_token(&normalise(password));
    let row: Option<(Uuid, Uuid, Option<DateTime<Utc>>)> = sqlx::query!(r#"SELECT a.id, u.id AS "c1", a.last_used_at FROM app_passwords a JOIN users u ON u.id = a.user_id
         WHERE a.token_hash = $1 AND lower(u.email) = lower($2) AND u.is_active"#, &hash, email.trim()).map(|r| (r.id, r.c1, r.last_used_at))
    .fetch_optional(&state.db)
    .await?;
    let Some((id, user, last)) = row else {
        return Ok(None);
    };
    // Note the use at most once a minute.
    if last.is_none_or(|l| Utc::now() - l > chrono::Duration::minutes(1)) {
        sqlx::query!("UPDATE app_passwords SET last_used_at = now(), last_used_ip = $2 WHERE id = $1", id, ip).execute(&state.db).await?;
    }
    Ok(Some(user))
}

async fn list(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    let rows: Vec<(Uuid, String, DateTime<Utc>, Option<DateTime<Utc>>, Option<String>)> =
        sqlx::query!(r#"SELECT id, name, created_at, last_used_at, last_used_ip FROM app_passwords WHERE user_id = $1 ORDER BY created_at DESC"#, auth.id).map(|r| (r.id, r.name, r.created_at, r.last_used_at, r.last_used_ip)).fetch_all(&state.db).await?;
    Ok(Json(json!({
        "items": rows.into_iter().map(|(id, name, created, used, ip)| json!({ "id": id, "name": name, "created_at": created, "last_used_at": used, "last_used_ip": ip })).collect::<Vec<_>>(),
    })))
}

#[derive(Deserialize)]
struct NewPassword {
    name: String,
}

/// Makes one; the password is in this answer only.
async fn create(State(state): State<AppState>, auth: AuthUser, ClientIp(ip): ClientIp, Json(b): Json<NewPassword>) -> AppResult<Json<Value>> {
    let name = required("name", &b.name, 100)?;
    let n: i64 = sqlx::query_scalar!(r#"SELECT COUNT(*) AS "v!" FROM app_passwords WHERE user_id = $1"#, auth.id).fetch_one(&state.db).await?;
    if n >= 20 {
        return Err(AppError::bad("at most 20 app passwords; revoke one you don't use"));
    }
    let password = new_password();
    let id: Uuid = sqlx::query_scalar!("INSERT INTO app_passwords (user_id, name, token_hash) VALUES ($1, $2, $3) RETURNING id", auth.id, &name, hash_token(&normalise(&password)))
        .fetch_one(&state.db)
        .await?;
    audit::record(&state.db, Some(auth.id), "auth.app_password_created", "user", Some(auth.id.to_string()), json!({ "name": name }), ip.as_deref()).await;
    Ok(Json(json!({ "id": id, "name": name, "password": password })))
}

async fn revoke(State(state): State<AppState>, auth: AuthUser, ClientIp(ip): ClientIp, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let name: Option<String> = sqlx::query_scalar!("DELETE FROM app_passwords WHERE id = $1 AND user_id = $2 RETURNING name", id, auth.id).fetch_optional(&state.db).await?;
    let name = name.ok_or(AppError::NotFound("app password"))?;
    audit::record(&state.db, Some(auth.id), "auth.app_password_revoked", "user", Some(auth.id.to_string()), json!({ "name": name }), ip.as_deref()).await;
    Ok(Json(json!({ "ok": true })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passwords_look_right_and_typing_is_forgiving() {
        let p = new_password();
        assert_eq!(p.len(), 19);
        assert!(p.split('-').all(|g| g.len() == 4 && g.chars().all(|c| c.is_ascii_lowercase())));
        assert_eq!(normalise("ABCD efgh-ijkl mnop"), "abcdefghijklmnop");
    }
}
