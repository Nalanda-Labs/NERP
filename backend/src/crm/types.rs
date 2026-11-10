//! Smart processes: record types defined by CRM admins. Each runs on the generic item
//! engine with its own pipelines and stages, custom fields, access rights, automation and
//! document templates. Options say whether it has an amount with products and whether it
//! is linked to a company and contacts.

use axum::{
    extract::{Path, State},
    routing::{patch, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};

use super::{access::is_crm_admin, entity_type, EntityType};
use crate::{
    auth::AuthUser,
    error::{AppError, AppResult},
    routes::required,
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/crm/types", post(create))
        .route("/crm/types/{code}", patch(update).delete(remove))
}

/// Icons a type can use in the sidebar (names from the frontend icon set).
const ICONS: &[&str] = &["folder", "file", "briefcase", "calendar", "clock", "list", "kanban", "globe", "building", "user", "send", "funnel", "key", "cpu", "mail"];

/// First path segments under /crm (frontend and API) a type code must not take.
const RESERVED: &[&str] = &[
    "lead", "deal", "contact", "company", "activities", "products", "settings", "recycle-bin", "items", "config",
    "counters", "lookup", "categories", "stages", "currencies", "permissions", "doc-templates", "documents",
    "requisites", "robots", "triggers", "automation", "types", "import", "export", "imports", "saved-filters", "fields",
    "timeline", "new", "bulk", "quote", "invoice", "quotes", "invoices", "order", "orders", "shop", "robot-runs", "schedules", "recurring", "robot-order", "card-layout", "email-templates", "marketing", "analytics", "targets", "webforms", "mailboxes", "merge", "duplicates", "kanban", "filters",
];

fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_end_matches('-').chars().take(40).collect()
}

fn require_admin(auth: &AuthUser) -> AppResult<()> {
    if is_crm_admin(auth) { Ok(()) } else { Err(AppError::Forbidden) }
}

fn icon(i: Option<&str>) -> AppResult<String> {
    let i = i.unwrap_or("folder");
    if ICONS.contains(&i) { Ok(i.into()) } else { Err(AppError::bad(format!("icon must be one of {}", ICONS.join(", ")))) }
}

#[derive(Deserialize)]
struct Body {
    name: Option<String>,
    name_plural: Option<String>,
    icon: Option<String>,
    has_pipelines: Option<bool>,
    has_amount: Option<bool>,
    has_client: Option<bool>,
}

async fn load(state: &AppState, id: i16) -> AppResult<EntityType> {
    super::entity_type_by_id(state, id).await
}

fn describe(t: &EntityType) -> Value {
    json!({
        "id": t.id, "code": t.code, "name": t.name, "name_plural": t.name_plural, "icon": t.icon,
        "has_pipelines": t.has_pipelines, "has_stages": t.has_stages, "has_amount": t.has_amount, "has_client": t.has_client,
        "is_system": t.is_system, "kind": t.kind,
    })
}

async fn create(State(state): State<AppState>, auth: AuthUser, Json(b): Json<Body>) -> AppResult<Json<Value>> {
    require_admin(&auth)?;
    let name = required("name", b.name.as_deref().unwrap_or(""), 60)?;
    let plural = required("plural name", b.name_plural.as_deref().unwrap_or(&name), 60)?;
    let icon = icon(b.icon.as_deref())?;
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM crm_entity_types WHERE NOT is_system").fetch_one(&state.db).await?;
    if n >= 100 {
        return Err(AppError::bad("at most 100 smart processes"));
    }
    // A readable, stable URL code: "contracts", "shipments", else "type-<id>".
    let mut tx = state.db.begin().await?;
    let id: i16 = sqlx::query_scalar("SELECT (GREATEST(127, COALESCE(MAX(id), 0)) + 1)::smallint FROM crm_entity_types").fetch_one(&mut *tx).await?;
    let base = slug(&plural);
    let mut code = if base.len() >= 2 && !RESERVED.contains(&base.as_str()) && !base.chars().next().unwrap().is_ascii_digit() { base } else { format!("type-{id}") };
    let taken: Vec<String> = sqlx::query_scalar("SELECT code FROM crm_entity_types").fetch_all(&mut *tx).await?;
    if taken.contains(&code) {
        code = format!("{code}-{id}");
    }
    sqlx::query(
        "INSERT INTO crm_entity_types (id, code, name, name_plural, has_pipelines, has_stages, has_amount, has_client, is_system, icon, sort)
         VALUES ($1, $2, $3, $4, $5, TRUE, $6, $7, FALSE, $8, 100 + $1)",
    )
    .bind(id)
    .bind(&code)
    .bind(&name)
    .bind(&plural)
    .bind(b.has_pipelines.unwrap_or(false))
    .bind(b.has_amount.unwrap_or(false))
    .bind(b.has_client.unwrap_or(true))
    .bind(&icon)
    .execute(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO crm_counters (entity_type) VALUES ($1)").bind(id).execute(&mut *tx).await?;
    let cat: uuid::Uuid = sqlx::query_scalar("INSERT INTO crm_categories (entity_type, name, sort, is_default) VALUES ($1, $2, 10, TRUE) RETURNING id")
        .bind(id)
        .bind(&plural)
        .fetch_one(&mut *tx)
        .await?;
    for (i, (stage, color, semantics)) in [("New", "#3b82f6", "process"), ("In progress", "#f59e0b", "process"), ("Done", "#22c55e", "success"), ("Cancelled", "#ef4444", "failure")].iter().enumerate() {
        sqlx::query("INSERT INTO crm_stages (category_id, name, color, sort, semantics) VALUES ($1, $2, $3, $4, $5)")
            .bind(cat)
            .bind(stage)
            .bind(color)
            .bind((i as i32 + 1) * 10)
            .bind(semantics)
            .execute(&mut *tx)
            .await?;
    }
    // Same access rights as deals to start with; admins adjust them under Access rights.
    sqlx::query("INSERT INTO crm_role_permissions (role_id, entity_type, action, scope) SELECT role_id, $1, action, scope FROM crm_role_permissions WHERE entity_type = 2")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    crate::audit::record(&state.db, Some(auth.id), "crm.type.create", "crm_type", Some(code.clone()), json!({ "name": name }), None).await;
    Ok(Json(describe(&load(&state, id).await?)))
}

async fn custom(state: &AppState, code: &str) -> AppResult<EntityType> {
    let t = entity_type(state, code).await?;
    if t.is_system {
        return Err(AppError::bad("built-in record types cannot be changed here"));
    }
    Ok(t)
}

async fn update(State(state): State<AppState>, auth: AuthUser, Path(code): Path<String>, Json(b): Json<Body>) -> AppResult<Json<Value>> {
    require_admin(&auth)?;
    let t = custom(&state, &code).await?;
    if t.kind.is_some() && (b.has_amount == Some(false) || b.has_client == Some(false)) {
        return Err(AppError::bad(format!("{} always have an amount and a client", t.name_plural.to_lowercase())));
    }
    if b.has_pipelines == Some(false) && t.has_pipelines {
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM crm_categories WHERE entity_type = $1").bind(t.id).fetch_one(&state.db).await?;
        if n > 1 {
            return Err(AppError::bad("delete the extra pipelines first"));
        }
    }
    sqlx::query(
        "UPDATE crm_entity_types SET name = $2, name_plural = $3, icon = $4, has_pipelines = $5, has_amount = $6, has_client = $7 WHERE id = $1",
    )
    .bind(t.id)
    .bind(match &b.name { Some(n) => required("name", n, 60)?, None => t.name.clone() })
    .bind(match &b.name_plural { Some(n) => required("plural name", n, 60)?, None => t.name_plural.clone() })
    .bind(match &b.icon { Some(i) => icon(Some(i))?, None => t.icon.clone() })
    .bind(b.has_pipelines.unwrap_or(t.has_pipelines))
    .bind(b.has_amount.unwrap_or(t.has_amount))
    .bind(b.has_client.unwrap_or(t.has_client))
    .execute(&state.db)
    .await?;
    Ok(Json(describe(&load(&state, t.id).await?)))
}

/// Deletes a smart process with its configuration. Only possible without records
/// (including the recycle bin), so no data is ever lost this way.
async fn remove(State(state): State<AppState>, auth: AuthUser, Path(code): Path<String>) -> AppResult<Json<Value>> {
    require_admin(&auth)?;
    let t = custom(&state, &code).await?;
    if t.kind.is_some() {
        return Err(AppError::bad(format!("{} are built in and cannot be deleted", t.name_plural.to_lowercase())));
    }
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM crm_items WHERE entity_type = $1").bind(t.id).fetch_one(&state.db).await?;
    if n > 0 {
        return Err(AppError::bad(format!("delete its {n} records first (including the recycle bin)")));
    }
    let mut tx = state.db.begin().await?;
    for sql in [
        "DELETE FROM crm_robots WHERE entity_type = $1",
        "DELETE FROM crm_triggers WHERE entity_type = $1",
        "DELETE FROM crm_categories WHERE entity_type = $1",
        "DELETE FROM crm_field_defs WHERE entity_type = $1",
        "DELETE FROM crm_role_permissions WHERE entity_type = $1",
        "DELETE FROM crm_saved_filters WHERE entity_type = $1",
        "DELETE FROM crm_counters WHERE entity_type = $1",
        "UPDATE crm_doc_templates SET entity_types = array_remove(entity_types, $1)",
        "DELETE FROM crm_entity_types WHERE id = $1",
    ] {
        sqlx::query(sql).bind(t.id).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    crate::audit::record(&state.db, Some(auth.id), "crm.type.delete", "crm_type", Some(t.code.clone()), json!({ "name": t.name }), None).await;
    Ok(Json(json!({ "ok": true })))
}

#[cfg(test)]
mod tests {
    use super::slug;

    #[test]
    fn slugs() {
        assert_eq!(slug("Contracts"), "contracts");
        assert_eq!(slug("Service Requests!"), "service-requests");
        assert_eq!(slug("  Ремонт  "), "");
    }
}
