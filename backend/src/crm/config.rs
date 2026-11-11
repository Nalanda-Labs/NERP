//! CRM configuration: pipelines (categories) and their stages, plus the combined config
//! document the UI loads once (types, pipelines, stages, fields, currencies, rights).

use axum::{
    extract::{Path, Query, State},
    routing::{get, patch, post, put},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::FromRow;
use uuid::Uuid;

use super::{
    access::{is_crm_admin, rights},
    entity_type,
    fields::{system_fields_for, system_options},
    EntityType,
};
use crate::{
    auth::AuthUser,
    error::{AppError, AppResult},
    routes::required,
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/crm/config", get(config))
        .route("/crm/traffic-sources", get(list_sources).post(create_source))
        .route("/crm/traffic-sources/{id}", axum::routing::patch(update_source).delete(delete_source))
        .route("/crm/{type}/categories", post(create_category))
        .route("/crm/categories/{id}", patch(update_category).delete(delete_category))
        .route("/crm/categories/{id}/stages", post(create_stage))
        .route("/crm/categories/{id}/stage-order", put(order_stages))
        .route("/crm/stages/{id}", patch(update_stage).delete(delete_stage))
}

#[derive(Serialize, FromRow, Clone)]
pub struct Stage {
    pub id: Uuid,
    pub category_id: Uuid,
    pub name: String,
    pub color: String,
    pub sort: i32,
    pub semantics: String,
    /// Time limit for records in this stage, in minutes.
    pub sla_minutes: Option<i32>,
    pub sla_escalate: bool,
    /// Fields a record needs before it enters this stage: standard ones by name, custom ones
    /// as "uf.<code>".
    pub required_fields: Vec<String>,
}

#[derive(Serialize, FromRow)]
pub struct Category {
    pub id: Uuid,
    pub name: String,
    pub sort: i32,
    pub is_default: bool,
    #[sqlx(skip)]
    pub stages: Vec<Stage>,
}

pub async fn categories(state: &AppState, entity_type: i16) -> AppResult<Vec<Category>> {
    let mut cats: Vec<Category> = sqlx::query_as("SELECT id, name, sort, is_default FROM crm_categories WHERE entity_type = $1 ORDER BY sort, created_at")
        .bind(entity_type)
        .fetch_all(&state.db)
        .await?;
    let ids: Vec<Uuid> = cats.iter().map(|c| c.id).collect();
    let stages: Vec<Stage> = sqlx::query_as(
        "SELECT id, category_id, name, color, sort, semantics, sla_minutes, sla_escalate, required_fields FROM crm_stages WHERE category_id = ANY($1) ORDER BY sort, name",
    )
    .bind(&ids)
    .fetch_all(&state.db)
    .await?;
    for c in &mut cats {
        c.stages = stages.iter().filter(|s| s.category_id == c.id).cloned().collect();
    }
    Ok(cats)
}

pub async fn stage(state: &AppState, id: Uuid) -> AppResult<Stage> {
    sqlx::query_as("SELECT id, category_id, name, color, sort, semantics, sla_minutes, sla_escalate, required_fields FROM crm_stages WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::bad("unknown stage"))
}

/// The default pipeline of a type, and its first open stage.
pub async fn default_placement(state: &AppState, t: &EntityType) -> AppResult<(Option<Uuid>, Option<Uuid>)> {
    if !t.has_stages {
        return Ok((None, None));
    }
    let row: Option<(Uuid, Uuid)> = sqlx::query_as(
        "SELECT c.id, s.id FROM crm_categories c JOIN crm_stages s ON s.category_id = c.id
         WHERE c.entity_type = $1 AND s.semantics = 'process'
         ORDER BY c.is_default DESC, c.sort, s.sort LIMIT 1",
    )
    .bind(t.id)
    .fetch_optional(&state.db)
    .await?;
    Ok(row.map(|(c, s)| (Some(c), Some(s))).unwrap_or((None, None)))
}

#[derive(Deserialize)]
struct ConfigQuery {}

async fn config(State(state): State<AppState>, auth: AuthUser, Query(_): Query<ConfigQuery>) -> AppResult<Json<Value>> {
    let types: Vec<EntityType> = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {} FROM crm_entity_types ORDER BY sort, id", super::TYPE_COLUMNS)))
        .fetch_all(&state.db)
        .await?;
    let mut out = Vec::new();
    for t in types {
        out.push(json!({
            "id": t.id,
            "code": t.code,
            "name": t.name,
            "name_plural": t.name_plural,
            "has_pipelines": t.has_pipelines,
            "has_stages": t.has_stages,
            "has_amount": t.has_amount,
            "has_client": t.has_client,
            "is_system": t.is_system,
            "kind": t.kind,
            "icon": t.icon,
            "categories": categories(&state, t.id).await?,
            "system_fields": system_fields_for(&t),
            "custom_fields": super::fields::visible_fields(&state, &auth, t.id).await?,
            "rights": rights(&state, &auth, t.id).await?,
        }));
    }
    let own_sources: Vec<(String, String)> = sqlx::query_as("SELECT code, name FROM crm_traffic_sources ORDER BY lower(name)").fetch_all(&state.db).await?;
    let mut options = system_options();
    if let Some(list) = options["source"].as_array_mut() {
        list.extend(own_sources.iter().map(|(c, _)| json!(c)));
    }
    let source_labels: serde_json::Map<String, Value> = own_sources.iter().map(|(c, n)| (c.clone(), json!(n))).collect();
    let currencies: Vec<(String, String, bool)> = sqlx::query_as("SELECT code, rate::text, is_base FROM crm_currencies ORDER BY is_base DESC, sort, code")
        .fetch_all(&state.db)
        .await?;
    Ok(Json(json!({
        "types": out,
        "currencies": currencies.iter().map(|(c, r, b)| json!({ "code": c, "rate": r, "is_base": b })).collect::<Vec<_>>(),
        "doc_engines": super::docs::engines(),
        "base_currency": currencies.iter().find(|c| c.2).map(|c| c.0.clone()),
        "options": options,
        "source_labels": source_labels,
        "is_admin": is_crm_admin(&auth),
        "can_manage_products": auth.can(crate::auth::permissions::CRM_PRODUCTS) || is_crm_admin(&auth),
    })))
}

fn require_admin(auth: &AuthUser) -> AppResult<()> {
    if is_crm_admin(auth) { Ok(()) } else { Err(AppError::Forbidden) }
}

fn check_color(c: &str) -> AppResult<()> {
    let ok = c.len() == 7 && c.starts_with('#') && c[1..].chars().all(|ch| ch.is_ascii_hexdigit());
    if ok { Ok(()) } else { Err(AppError::bad("colour must look like #3b82f6")) }
}

fn check_semantics(s: &str) -> AppResult<()> {
    if ["process", "success", "failure"].contains(&s) { Ok(()) } else { Err(AppError::bad("semantics must be process, success or failure")) }
}

#[derive(Deserialize)]
struct NewCategory {
    name: String,
}

/// New pipeline, starting with the same stages as the default pipeline.
async fn create_category(State(state): State<AppState>, auth: AuthUser, Path(code): Path<String>, Json(body): Json<NewCategory>) -> AppResult<Json<Value>> {
    require_admin(&auth)?;
    let t = entity_type(&state, &code).await?;
    if !t.has_pipelines {
        return Err(AppError::bad(format!("{} have a single pipeline", t.name_plural.to_lowercase())));
    }
    let name = required("name", &body.name, 100)?;
    let mut tx = state.db.begin().await?;
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO crm_categories (entity_type, name, sort) VALUES ($1, $2, COALESCE((SELECT MAX(sort) FROM crm_categories WHERE entity_type = $1), 0) + 10) RETURNING id",
    )
    .bind(t.id)
    .bind(&name)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO crm_stages (category_id, name, color, sort, semantics)
         SELECT $1, s.name, s.color, s.sort, s.semantics FROM crm_stages s
         JOIN crm_categories c ON c.id = s.category_id WHERE c.entity_type = $2 AND c.is_default",
    )
    .bind(id)
    .bind(t.id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(json!({ "categories": categories(&state, t.id).await? })))
}

async fn category_type(state: &AppState, id: Uuid) -> AppResult<(i16, bool)> {
    sqlx::query_as("SELECT entity_type, is_default FROM crm_categories WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound("pipeline"))
}

#[derive(Deserialize)]
struct UpdateCategory {
    name: Option<String>,
    sort: Option<i32>,
    is_default: Option<bool>,
}

async fn update_category(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(body): Json<UpdateCategory>) -> AppResult<Json<Value>> {
    require_admin(&auth)?;
    let (t, _) = category_type(&state, id).await?;
    if let Some(n) = &body.name {
        sqlx::query("UPDATE crm_categories SET name = $1 WHERE id = $2").bind(required("name", n, 100)?).bind(id).execute(&state.db).await?;
    }
    if let Some(s) = body.sort {
        sqlx::query("UPDATE crm_categories SET sort = $1 WHERE id = $2").bind(s).bind(id).execute(&state.db).await?;
    }
    if body.is_default == Some(true) {
        let mut tx = state.db.begin().await?;
        sqlx::query("UPDATE crm_categories SET is_default = FALSE WHERE entity_type = $1").bind(t).execute(&mut *tx).await?;
        sqlx::query("UPDATE crm_categories SET is_default = TRUE WHERE id = $1").bind(id).execute(&mut *tx).await?;
        tx.commit().await?;
    }
    Ok(Json(json!({ "categories": categories(&state, t).await? })))
}

async fn delete_category(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    require_admin(&auth)?;
    let (t, is_default) = category_type(&state, id).await?;
    if is_default {
        return Err(AppError::bad("the default pipeline cannot be deleted; make another one the default first"));
    }
    let used: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM crm_items WHERE category_id = $1)")
        .bind(id)
        .fetch_one(&state.db)
        .await?;
    if used {
        return Err(AppError::bad("this pipeline still has records (including in the recycle bin); move them to another pipeline first"));
    }
    sqlx::query("DELETE FROM crm_categories WHERE id = $1").bind(id).execute(&state.db).await?;
    Ok(Json(json!({ "categories": categories(&state, t).await? })))
}

#[derive(Deserialize)]
struct NewStage {
    name: String,
    color: Option<String>,
    semantics: Option<String>,
}

async fn create_stage(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(body): Json<NewStage>) -> AppResult<Json<Value>> {
    require_admin(&auth)?;
    let (t, _) = category_type(&state, id).await?;
    let name = required("name", &body.name, 60)?;
    let color = body.color.unwrap_or_else(|| "#94a3b8".into());
    check_color(&color)?;
    let semantics = body.semantics.unwrap_or_else(|| "process".into());
    check_semantics(&semantics)?;
    // New open stages go after the last open stage; closing stages go at the end.
    sqlx::query(
        "INSERT INTO crm_stages (category_id, name, color, semantics, sort)
         VALUES ($1, $2, $3, $4, COALESCE((SELECT MAX(sort) FROM crm_stages WHERE category_id = $1 AND (semantics = 'process' OR $4 <> 'process')), 0) + 5)",
    )
    .bind(id)
    .bind(name)
    .bind(color)
    .bind(semantics)
    .execute(&state.db)
    .await?;
    Ok(Json(json!({ "categories": categories(&state, t).await? })))
}

#[derive(Deserialize)]
struct UpdateStage {
    name: Option<String>,
    color: Option<String>,
    /// Minutes; 0 or null removes the limit (send the key to change it).
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    sla_minutes: Option<Option<i32>>,
    sla_escalate: Option<bool>,
    required_fields: Option<Vec<String>>,
}

async fn update_stage(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(body): Json<UpdateStage>) -> AppResult<Json<Value>> {
    require_admin(&auth)?;
    let s = stage(&state, id).await?;
    let (t, _) = category_type(&state, s.category_id).await?;
    if let Some(n) = &body.name {
        sqlx::query("UPDATE crm_stages SET name = $1 WHERE id = $2").bind(required("name", n, 60)?).bind(id).execute(&state.db).await?;
    }
    if let Some(c) = &body.color {
        check_color(c)?;
        sqlx::query("UPDATE crm_stages SET color = $1 WHERE id = $2").bind(c).bind(id).execute(&state.db).await?;
    }
    if let Some(m) = body.sla_minutes {
        let m = m.filter(|m| *m != 0);
        if m.is_some_and(|m| !(1..=525_600).contains(&m)) {
            return Err(AppError::bad("the time limit must be between 1 minute and 365 days"));
        }
        if m.is_some() && s.semantics != "process" {
            return Err(AppError::bad("only open stages can have a time limit"));
        }
        sqlx::query("UPDATE crm_stages SET sla_minutes = $1 WHERE id = $2").bind(m).bind(id).execute(&state.db).await?;
    }
    if let Some(e) = body.sla_escalate {
        sqlx::query("UPDATE crm_stages SET sla_escalate = $1 WHERE id = $2").bind(e).bind(id).execute(&state.db).await?;
    }
    if let Some(f) = &body.required_fields {
        let ok = |c: &str| super::items::REQUIRABLE.iter().any(|(k, _)| *k == c) || c.strip_prefix("uf.").is_some_and(|u| !u.is_empty() && u.len() <= 64 && u.chars().all(|x| x.is_ascii_alphanumeric() || x == '_'));
        if f.len() > 40 || f.iter().any(|c| !ok(c)) {
            return Err(AppError::bad("unknown field among the required ones"));
        }
        let mut f = f.clone();
        f.dedup();
        sqlx::query("UPDATE crm_stages SET required_fields = $1 WHERE id = $2").bind(&f).bind(id).execute(&state.db).await?;
    }
    Ok(Json(json!({ "categories": categories(&state, t).await? })))
}

#[derive(Deserialize)]
struct StageOrder {
    ids: Vec<Uuid>,
}

async fn order_stages(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(body): Json<StageOrder>) -> AppResult<Json<Value>> {
    require_admin(&auth)?;
    let (t, _) = category_type(&state, id).await?;
    let mut tx = state.db.begin().await?;
    for (i, sid) in body.ids.iter().enumerate() {
        sqlx::query("UPDATE crm_stages SET sort = $1 WHERE id = $2 AND category_id = $3")
            .bind((i as i32 + 1) * 10)
            .bind(sid)
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(Json(json!({ "categories": categories(&state, t).await? })))
}

#[derive(Deserialize)]
struct DeleteStage {
    move_to: Option<Uuid>,
}

/// Deletes a stage, moving its records to another stage of the same pipeline. A pipeline
/// must keep at least one open, one successful and one failed stage.
async fn delete_stage(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Query(q): Query<DeleteStage>) -> AppResult<Json<Value>> {
    require_admin(&auth)?;
    let s = stage(&state, id).await?;
    let (t, _) = category_type(&state, s.category_id).await?;
    let same: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM crm_stages WHERE category_id = $1 AND semantics = $2")
        .bind(s.category_id)
        .bind(&s.semantics)
        .fetch_one(&state.db)
        .await?;
    if same <= 1 {
        return Err(AppError::bad(match s.semantics.as_str() {
            "success" => "a pipeline needs at least one successful stage",
            "failure" => "a pipeline needs at least one failed stage",
            _ => "a pipeline needs at least one open stage",
        }));
    }
    let target = match q.move_to {
        Some(m) => {
            let ts = stage(&state, m).await?;
            if ts.category_id != s.category_id || ts.id == id {
                return Err(AppError::bad("move records to another stage of the same pipeline"));
            }
            ts
        }
        None => {
            let first: Uuid = sqlx::query_scalar("SELECT id FROM crm_stages WHERE category_id = $1 AND id <> $2 AND semantics = $3 ORDER BY sort LIMIT 1")
                .bind(s.category_id)
                .bind(id)
                .bind(&s.semantics)
                .fetch_one(&state.db)
                .await?;
            stage(&state, first).await?
        }
    };
    let mut tx = state.db.begin().await?;
    // Records moved off the deleted stage enter the target stage (for the funnel).
    sqlx::query(
        "INSERT INTO crm_stage_history (item_id, category_id, stage_id, semantics) SELECT id, $2, $1, $3 FROM crm_items WHERE stage_id = $4",
    )
    .bind(target.id)
    .bind(target.category_id)
    .bind(&target.semantics)
    .bind(id)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE crm_items SET stage_id = $1, stage_semantics = $2, is_closed = ($2 <> 'process') WHERE stage_id = $3")
        .bind(target.id)
        .bind(&target.semantics)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM crm_stages WHERE id = $1").bind(id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(json!({ "categories": categories(&state, t).await? })))
}

// ---------------------------------------------------------------------------
// The company's own traffic sources
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct SourceInput {
    name: Option<String>,
    /// utm_source values that mean this source.
    utm: Option<Vec<String>>,
}

async fn sources_json(state: &AppState) -> AppResult<Json<Value>> {
    let rows: Vec<(Uuid, String, String, Vec<String>)> = sqlx::query_as("SELECT id, code, name, utm FROM crm_traffic_sources ORDER BY lower(name)").fetch_all(&state.db).await?;
    Ok(Json(json!({ "items": rows.into_iter().map(|(id, c, n, u)| json!({ "id": id, "code": c, "name": n, "utm": u })).collect::<Vec<_>>() })))
}

fn clean_utm(list: &[String]) -> AppResult<Vec<String>> {
    let mut out: Vec<String> = list.iter().map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()).collect();
    out.sort();
    out.dedup();
    if out.len() > 50 || out.iter().any(|s| s.len() > 100) {
        return Err(AppError::bad("up to 50 utm_source values of up to 100 characters"));
    }
    Ok(out)
}

async fn list_sources(State(state): State<AppState>, _auth: AuthUser) -> AppResult<Json<Value>> {
    sources_json(&state).await
}

async fn create_source(State(state): State<AppState>, auth: AuthUser, Json(b): Json<SourceInput>) -> AppResult<Json<Value>> {
    require_admin(&auth)?;
    let name = crate::routes::required("name", b.name.as_deref().unwrap_or(""), 60)?;
    let slug: String = name.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect::<String>().split('_').filter(|s| !s.is_empty()).collect::<Vec<_>>().join("_");
    let code = format!("c_{}", if slug.is_empty() { "source".into() } else { slug.chars().take(40).collect::<String>() });
    if system_options()["source"].as_array().is_some_and(|a| a.iter().any(|x| x == &json!(code))) {
        return Err(AppError::bad("that source is built in"));
    }
    sqlx::query("INSERT INTO crm_traffic_sources (code, name, utm) VALUES ($1, $2, $3)")
        .bind(&code)
        .bind(&name)
        .bind(clean_utm(&b.utm.unwrap_or_default())?)
        .execute(&state.db)
        .await
        .map_err(|e| if crate::error::is_unique_violation(&e) { AppError::Conflict("a source with that name exists".into()) } else { e.into() })?;
    sources_json(&state).await
}

async fn update_source(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<SourceInput>) -> AppResult<Json<Value>> {
    require_admin(&auth)?;
    let name = b.name.as_deref().map(|n| crate::routes::required("name", n, 60)).transpose()?;
    let utm = b.utm.as_deref().map(clean_utm).transpose()?;
    let n = sqlx::query("UPDATE crm_traffic_sources SET name = COALESCE($2, name), utm = COALESCE($3, utm) WHERE id = $1").bind(id).bind(name).bind(utm).execute(&state.db).await?.rows_affected();
    if n == 0 {
        return Err(AppError::NotFound("source"));
    }
    sources_json(&state).await
}

/// Records keep the source's code; it shows as the code once the source is gone.
async fn delete_source(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    require_admin(&auth)?;
    sqlx::query("DELETE FROM crm_traffic_sources WHERE id = $1").bind(id).execute(&state.db).await?;
    sources_json(&state).await
}
