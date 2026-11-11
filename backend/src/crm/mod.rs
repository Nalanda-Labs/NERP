//! CRM module (phase 1): leads, deals, contacts and companies on one generic item engine.

pub mod access;
mod activities;
mod analytics;
mod cards;
pub mod collab;
pub mod recurring;
pub mod catalog;
pub mod esign;
pub mod sign_templates;
pub mod automation;
mod config;
pub mod docs;
pub mod email;
mod fields;
pub(crate) mod items;
pub(crate) mod money;
mod permissions;
pub mod requisites;
mod timeline;
mod tools;
mod types;
mod webforms;

use axum::Router;
use serde::Serialize;
use serde_json::{json, Value};
use sqlx::FromRow;
use uuid::Uuid;

use crate::{
    error::{AppError, AppResult},
    realtime,
    state::AppState,
};

pub use activities::REMIND_JOB;
pub use fields::ufield_text;
pub use tools::PURGE_JOB;

pub fn router() -> Router<AppState> {
    Router::new()
        .merge(config::router())
        .merge(fields::router())
        .merge(items::router())
        .merge(timeline::router())
        .merge(activities::router())
        .merge(money::router())
        .merge(permissions::router())
        .merge(tools::router())
        .merge(docs::api::router())
        .merge(requisites::router())
        .merge(automation::router())
        .merge(types::router())
        .merge(email::router())
        .merge(webforms::router())
        .merge(analytics::router())
        .merge(cards::router())
        .merge(collab::router())
        .merge(recurring::router())
        .merge(catalog::router())
        .merge(esign::router())
        .merge(sign_templates::router())
}

pub async fn run_reminders(state: &AppState) -> anyhow::Result<()> {
    activities::run_reminders(state).await
}

pub async fn run_purge(state: &AppState) -> anyhow::Result<()> {
    tools::run_purge(state).await
}

pub async fn run_import(state: &AppState, payload: &Value) -> anyhow::Result<()> {
    tools::run_import(state, payload).await
}

pub use tools::IMPORT_JOB;

pub const LEAD: i16 = 1;
pub const DEAL: i16 = 2;
pub const CONTACT: i16 = 3;
pub const COMPANY: i16 = 4;

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct EntityType {
    pub id: i16,
    pub code: String,
    pub name: String,
    pub name_plural: String,
    pub has_pipelines: bool,
    pub has_stages: bool,
    pub has_amount: bool,
    /// Linked to a company and contacts (leads, deals, smart processes that choose it).
    pub has_client: bool,
    /// Built in (lead, deal, contact, company) rather than a smart process.
    pub is_system: bool,
    pub icon: String,
    /// "quote" or "invoice" for those built-in types (smart-process engine, not deletable).
    pub kind: Option<String>,
}

pub const TYPE_COLUMNS: &str = "id, code, name, name_plural, has_pipelines, has_stages, has_amount, has_client, is_system, icon, kind";

impl EntityType {
    pub fn is_person(&self) -> bool {
        self.id == CONTACT || self.id == LEAD
    }
    /// A smart process (defined by an admin).
    pub fn is_custom(&self) -> bool {
        !self.is_system
    }
}

pub async fn entity_type(state: &AppState, code: &str) -> AppResult<EntityType> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {TYPE_COLUMNS} FROM crm_entity_types WHERE code = $1")))
        .bind(code)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound("record type"))
}

pub async fn entity_type_by_id(state: &AppState, id: i16) -> AppResult<EntityType> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {TYPE_COLUMNS} FROM crm_entity_types WHERE id = $1")))
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound("record type"))
}

/// Writes one timeline entry.
pub async fn timeline<'e, E: sqlx::PgExecutor<'e>>(
    executor: E,
    item: Uuid,
    kind: &str,
    author: Option<Uuid>,
    body: Option<String>,
    data: Value,
) -> sqlx::Result<()> {
    sqlx::query("INSERT INTO crm_timeline (item_id, kind, author_id, body, data) VALUES ($1, $2, $3, $4, $5)")
        .bind(item)
        .bind(kind)
        .bind(author)
        .bind(body)
        .bind(data)
        .execute(executor)
        .await?;
    Ok(())
}

/// Tells open CRM screens that a record changed, so lists, boards and detail pages refresh.
pub async fn broadcast(state: &AppState, entity_type: i16, id: Uuid, deleted: bool) {
    realtime::publish(state, None, "crm.changed", json!({ "entity_type": entity_type, "id": id, "deleted": deleted })).await;
}

/// Lowercase text indexed for search: names, title, company, contact details.
pub fn search_text(parts: &[Option<&str>], multi: &[(String, String)]) -> String {
    let mut out = String::new();
    for p in parts.iter().flatten() {
        out.push_str(&p.to_lowercase());
        out.push(' ');
    }
    for (value, normalized) in multi {
        out.push_str(&value.to_lowercase());
        out.push(' ');
        // Phones are also searchable as one run of digits, e.g. "15550107788".
        let digits: String = value.chars().filter(|c| c.is_ascii_digit()).collect();
        if digits.len() >= 5 {
            out.push_str(&digits);
            out.push(' ');
        }
        if normalized != &value.to_lowercase() && normalized != &digits {
            out.push_str(normalized);
            out.push(' ');
        }
    }
    out.chars().take(4000).collect()
}
