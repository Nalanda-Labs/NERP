pub mod admin;
pub mod audit;
pub mod auth;
pub mod departments;
pub mod files;
pub mod roles;
pub mod settings;
pub mod security;
pub mod setup;
pub mod users;

use axum::{extract::State, routing::get, Json, Router};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{json, Value};

use crate::{error::AppResult, state::AppState};

pub fn router(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/health", get(health))
        .merge(setup::router())
        .merge(auth::router())
        .merge(users::router())
        .merge(departments::router())
        .merge(roles::router())
        .merge(files::router(&state))
        .merge(settings::router())
        .merge(audit::router())
        .merge(security::router())
        .merge(admin::router())
        .merge(crate::notify::router())
        .merge(crate::realtime::router())
        .merge(crate::tasks::router())
        .merge(crate::crm::router())
        .merge(crate::api::router())
        .merge(crate::marketing::router())
        .merge(crate::shop::router())
        .merge(crate::inventory::router())
        .merge(crate::chat::router())
        .merge(crate::calendar::router())
        .merge(crate::im::router())
        .merge(crate::feed::router())
        .merge(crate::kb::router())
        .merge(crate::hr::router())
        .merge(crate::sites::router())
        .merge(crate::calls::router())
        .merge(crate::lists::router())
        .merge(crate::workflows::router())
        .merge(crate::directory::router())
        .merge(crate::accounting::router())
        .merge(crate::drive::router())
        .merge(crate::booking::router())
        .merge(crate::training::router())
        .merge(crate::support::router())
}

async fn health(State(state): State<AppState>) -> AppResult<Json<Value>> {
    sqlx::query("SELECT 1").execute(&state.db).await?;
    let mut redis = state.redis.clone();
    let _: String = redis::cmd("PING").query_async(&mut redis).await?;
    Ok(Json(json!({ "status": "ok" })))
}

#[derive(Debug, Deserialize)]
pub struct PageParams {
    #[serde(default, deserialize_with = "num_or_string")]
    pub limit: Option<i64>,
    #[serde(default, deserialize_with = "num_or_string")]
    pub offset: Option<i64>,
}

/// Query strings reach `#[serde(flatten)]`ed structs as strings, so accept both forms.
fn num_or_string<'de, D: Deserializer<'de>>(de: D) -> Result<Option<i64>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum N {
        I(i64),
        S(String),
    }
    match Option::<N>::deserialize(de)? {
        None => Ok(None),
        Some(N::I(i)) => Ok(Some(i)),
        Some(N::S(s)) if s.trim().is_empty() => Ok(None),
        Some(N::S(s)) => s.trim().parse().map(Some).map_err(serde::de::Error::custom),
    }
}

impl PageParams {
    pub fn limit(&self) -> i64 {
        self.limit.unwrap_or(50).clamp(1, 200)
    }
    pub fn offset(&self) -> i64 {
        self.offset.unwrap_or(0).max(0)
    }
}

#[derive(Serialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
}

/// For PATCH bodies: distinguishes "field absent" (`None`) from "set to null" (`Some(None)`).
pub fn double_option<'de, T, D>(de: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    Option::<T>::deserialize(de).map(Some)
}

/// Trims a string and turns empty strings into `None`.
pub fn clean(s: Option<String>) -> Option<String> {
    s.map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

/// Like `clean` but for a required field.
pub fn required(field: &str, value: &str, max: usize) -> AppResult<String> {
    let v = value.trim();
    if v.is_empty() {
        return Err(crate::error::AppError::bad(format!("{field} is required")));
    }
    if v.chars().count() > max {
        return Err(crate::error::AppError::bad(format!("{field} is too long (max {max})")));
    }
    Ok(v.to_string())
}

pub fn normalize_email(email: &str) -> AppResult<String> {
    let e = email.trim().to_lowercase();
    let valid = e.len() <= 254
        && e.split_once('@').is_some_and(|(local, domain)| {
            !local.is_empty() && domain.contains('.') && !domain.starts_with('.') && !domain.ends_with('.')
        })
        && !e.contains(char::is_whitespace)
        && !e.contains(char::is_control);
    if !valid {
        return Err(crate::error::AppError::bad("a valid email address is required"));
    }
    Ok(e)
}

/// A URL-friendly name: lowercase ASCII letters and digits joined by dashes, at most 40
/// characters; `fallback` when nothing is left.
pub fn slugify(name: &str, fallback: &str) -> String {
    let mut out = String::new();
    for c in name.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let s: String = out.chars().take(40).collect::<String>().trim_end_matches('-').to_string();
    if s.is_empty() { fallback.into() } else { s }
}
