//! How people arrange the app: sidebar items hidden or pinned, and the background. Everyone
//! sets their own; an administrator sets the company's default and can apply it to everyone
//! (which clears personal choices).

use axum::{extract::State, routing::{get, put}, Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{
    audit,
    auth::{permissions as perm, AuthUser, ClientIp},
    error::{AppError, AppResult},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/me/ui", get(get_mine).put(put_mine))
        .route("/settings/ui-default", put(put_default))
        .route("/me/tours", get(tours).delete(reset_tours))
        .route("/me/tours/{name}", axum::routing::post(tour_done))
}

const PRESETS: &[&str] = &["plain", "sky", "sand", "mint", "dusk", "grid"];

/// `{"hidden": [...], "pinned": [...]}` with app paths only.
fn check_menu(v: &Value) -> AppResult<Value> {
    let list = |k: &str| -> AppResult<Vec<String>> {
        let items = v.get(k).and_then(Value::as_array).cloned().unwrap_or_default();
        if items.len() > 200 {
            return Err(AppError::bad("too many menu items"));
        }
        items
            .iter()
            .map(|i| i.as_str().filter(|s| s.starts_with('/') && s.len() <= 200 && !s.contains("//")).map(String::from).ok_or_else(|| AppError::bad("menu items are app paths")))
            .collect()
    };
    Ok(json!({ "hidden": list("hidden")?, "pinned": list("pinned")? }))
}

fn check_background(b: &str) -> AppResult<()> {
    if PRESETS.contains(&b) || uuid::Uuid::parse_str(b).is_ok() { Ok(()) } else { Err(AppError::bad("pick a background or upload a picture")) }
}

async fn company(state: &AppState) -> AppResult<Value> {
    Ok(sqlx::query_scalar!("SELECT value FROM settings WHERE key = 'ui_default'").fetch_optional(&state.db).await?.unwrap_or(json!({})))
}

async fn get_mine(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    let mine: Option<(Option<Value>, Option<String>)> = sqlx::query!(r#"SELECT menu, background FROM user_ui_prefs WHERE user_id = $1"#, auth.id).map(|r| (r.menu, r.background)).fetch_optional(&state.db).await?;
    let (menu, background) = mine.unwrap_or((None, None));
    let c = company(&state).await?;
    Ok(Json(json!({
        "menu": menu.clone().or_else(|| c.get("menu").cloned()).unwrap_or(json!({ "hidden": [], "pinned": [] })),
        "background": background.clone().or_else(|| c.get("background").and_then(Value::as_str).map(String::from)).unwrap_or_else(|| "plain".into()),
        "own_menu": menu.is_some(), "own_background": background.is_some(),
    })))
}

#[derive(Deserialize)]
struct Mine {
    /// null: back to the company's.
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    menu: Option<Option<Value>>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    background: Option<Option<String>>,
}

async fn put_mine(State(state): State<AppState>, auth: AuthUser, Json(b): Json<Mine>) -> AppResult<Json<Value>> {
    let menu = match &b.menu {
        Some(Some(m)) => Some(Some(check_menu(m)?)),
        Some(None) => Some(None),
        None => None,
    };
    if let Some(Some(bg)) = &b.background {
        check_background(bg)?;
    }
    sqlx::query!("INSERT INTO user_ui_prefs (user_id, menu, background) VALUES ($1, $3, $5)
         ON CONFLICT (user_id) DO UPDATE SET menu = CASE WHEN $2 THEN $3 ELSE user_ui_prefs.menu END,
             background = CASE WHEN $4 THEN $5 ELSE user_ui_prefs.background END, updated_at = now()", auth.id, menu.is_some(), menu.flatten(), b.background.is_some(), b.background.flatten())
    .execute(&state.db)
    .await?;
    get_mine(State(state), auth).await
}

#[derive(Deserialize)]
struct Default {
    menu: Option<Value>,
    background: Option<String>,
    /// Also clear everyone's own choices, so they get these.
    #[serde(default)]
    apply_to_all: bool,
}

async fn put_default(State(state): State<AppState>, auth: AuthUser, ClientIp(ip): ClientIp, Json(b): Json<Default>) -> AppResult<Json<Value>> {
    auth.require(perm::SETTINGS_MANAGE)?;
    let mut c = company(&state).await?;
    if let Some(m) = &b.menu {
        c["menu"] = check_menu(m)?;
    }
    if let Some(bg) = &b.background {
        check_background(bg)?;
        c["background"] = json!(bg);
    }
    let mut tx = state.db.begin().await?;
    sqlx::query!("INSERT INTO settings (key, value) VALUES ('ui_default', $1) ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = now()", &c).execute(&mut *tx).await?;
    if b.apply_to_all {
        if b.menu.is_some() {
            sqlx::query!("UPDATE user_ui_prefs SET menu = NULL").execute(&mut *tx).await?;
        }
        if b.background.is_some() {
            sqlx::query!("UPDATE user_ui_prefs SET background = NULL").execute(&mut *tx).await?;
        }
    }
    audit::record(&mut *tx, Some(auth.id), "settings.ui_default", "settings", None, json!({ "menu": b.menu.is_some(), "background": b.background, "apply_to_all": b.apply_to_all }), ip.as_deref()).await;
    tx.commit().await?;
    get_mine(State(state), auth).await
}

// ---------------------------------------------------------------------------
// Guided tours
// ---------------------------------------------------------------------------

const TOURS: &[&str] = &["welcome", "crm", "tasks"];

async fn tours(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    let done: Vec<String> = sqlx::query_scalar!("SELECT tours_done FROM users WHERE id = $1", auth.id).fetch_one(&state.db).await?;
    Ok(Json(json!({ "done": done, "tours": TOURS })))
}

/// A tour seen to the end (or skipped): not offered again.
async fn tour_done(State(state): State<AppState>, auth: AuthUser, axum::extract::Path(name): axum::extract::Path<String>) -> AppResult<Json<Value>> {
    if !TOURS.contains(&name.as_str()) {
        return Err(AppError::NotFound("tour"));
    }
    sqlx::query!("UPDATE users SET tours_done = array_append(tours_done, $2) WHERE id = $1 AND NOT ($2 = ANY(tours_done))", auth.id, &name).execute(&state.db).await?;
    tours(State(state), auth).await
}

/// "Take the tours again".
async fn reset_tours(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    sqlx::query!("UPDATE users SET tours_done = '{}' WHERE id = $1", auth.id).execute(&state.db).await?;
    tours(State(state), auth).await
}
