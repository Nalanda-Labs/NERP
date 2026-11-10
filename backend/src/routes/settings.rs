use axum::{extract::State, routing::get, Json, Router};
use serde_json::{json, Map, Value};

use crate::{
    audit,
    auth::{permissions as perm, AuthUser, ClientIp},
    error::{AppError, AppResult},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new().route("/settings", get(get_settings).put(put_settings))
}

/// Known settings, their defaults, and a validator. New modules add their keys here.
fn defaults() -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("company_name".into(), json!("My Company"));
    m.insert("default_timezone".into(), json!("UTC"));
    m.insert("default_locale".into(), json!("en"));
    m.insert("date_format".into(), json!("YYYY-MM-DD"));
    m.insert("week_start".into(), json!("monday"));
    // Everyone must use a code from an authenticator app when signing in.
    m.insert("require_two_factor".into(), json!(false));
    // Seller details printed on quotes and invoices.
    for k in SELLER_KEYS {
        m.insert((*k).into(), json!(""));
    }
    m
}

pub const SELLER_KEYS: &[&str] = &[
    "company_legal_name", "company_address", "company_tax_id", "company_registration_no", "company_phone",
    "company_email", "company_website", "company_bank_details", "company_logo_file_id",
];

fn validate(key: &str, value: &Value) -> AppResult<()> {
    if key == "require_two_factor" {
        return if value.is_boolean() { Ok(()) } else { Err(AppError::bad("require_two_factor must be true or false")) };
    }
    if SELLER_KEYS.contains(&key) {
        let s = value.as_str().ok_or_else(|| AppError::bad(format!("{key} must be text")))?;
        if s.len() > 2000 {
            return Err(AppError::bad(format!("{key} is too long")));
        }
        if key == "company_logo_file_id" && !s.is_empty() && uuid::Uuid::parse_str(s).is_err() {
            return Err(AppError::bad("logo must be an uploaded file"));
        }
        return Ok(());
    }
    let s = value.as_str().ok_or_else(|| AppError::bad(format!("{key} must be a string")))?;
    let ok = match key {
        "company_name" => !s.trim().is_empty() && s.len() <= 200,
        "default_timezone" | "default_locale" => !s.is_empty() && s.len() <= 64,
        "date_format" => ["YYYY-MM-DD", "DD.MM.YYYY", "DD/MM/YYYY", "MM/DD/YYYY"].contains(&s),
        "week_start" => ["monday", "sunday"].contains(&s),
        _ => return Err(AppError::bad(format!("unknown setting: {key}"))),
    };
    if ok { Ok(()) } else { Err(AppError::bad(format!("invalid value for {key}"))) }
}

/// Whether the company requires two-step sign-in for everyone.
pub async fn two_factor_required(state: &AppState) -> AppResult<bool> {
    let v: Option<Value> = sqlx::query_scalar("SELECT value FROM settings WHERE key = 'require_two_factor'").fetch_optional(&state.db).await?;
    Ok(v.and_then(|v| v.as_bool()).unwrap_or(false))
}

pub(crate) async fn load(state: &AppState) -> AppResult<Map<String, Value>> {
    let mut out = defaults();
    let rows: Vec<(String, Value)> = sqlx::query_as("SELECT key, value FROM settings")
        .fetch_all(&state.db)
        .await?;
    for (k, v) in rows {
        if out.contains_key(&k) {
            out.insert(k, v);
        }
    }
    Ok(out)
}

async fn get_settings(State(state): State<AppState>, _auth: AuthUser) -> AppResult<Json<Map<String, Value>>> {
    Ok(Json(load(&state).await?))
}

async fn put_settings(
    State(state): State<AppState>,
    auth: AuthUser,
    ClientIp(ip): ClientIp,
    Json(body): Json<Map<String, Value>>,
) -> AppResult<Json<Map<String, Value>>> {
    auth.require(perm::SETTINGS_MANAGE)?;
    for (k, v) in &body {
        validate(k, v)?;
    }
    let mut tx = state.db.begin().await?;
    for (k, v) in &body {
        let v = match v {
            Value::String(s) => Value::String(s.trim().to_string()),
            other => other.clone(),
        };
        sqlx::query(
            "INSERT INTO settings (key, value) VALUES ($1, $2)
             ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = now()",
        )
        .bind(k)
        .bind(v)
        .execute(&mut *tx)
        .await?;
    }
    audit::record(&mut *tx, Some(auth.id), "settings.update", "settings", None, Value::Object(body), ip.as_deref()).await;
    tx.commit().await?;
    Ok(Json(load(&state).await?))
}
