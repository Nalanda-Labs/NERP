//! Sales generator: a deal (or lead) for every client in a segment of contacts or companies —
//! repeat sales, renewals, seasonal offers — made now or again every week, month or quarter.
//!
//! Each record goes to the client's responsible person, or to chosen people in turn, in the
//! chosen pipeline and stage, titled from a template. Clients who already have an open record
//! of that kind (in that pipeline) can be skipped. Needs "Run marketing".

use axum::{
    extract::{Path, State},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Datelike, Duration, Months, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sqlx::{FromRow, Postgres, QueryBuilder};
use uuid::Uuid;

use super::{load_segment, parse_filter, require, restrict, segment_type};
use crate::{
    auth::AuthUser,
    crm::{automation::system_auth, entity_type_by_id, items::create_item, COMPANY, CONTACT, DEAL, LEAD},
    error::{AppError, AppResult},
    jobs,
    routes::required,
    state::AppState,
};

pub const GENERATOR_JOB: &str = "marketing.generator";
/// Most clients one run goes through.
const MAX_CLIENTS: i64 = 10_000;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/marketing/generators", get(list).post(create))
        .route("/marketing/generators/{id}", get(get_one).patch(update).delete(remove))
        .route("/marketing/generators/{id}/run", post(run_now))
        .route("/marketing/generators/{id}/preview", get(preview))
}

#[derive(Serialize, FromRow, Clone)]
struct Generator {
    id: Uuid,
    name: String,
    segment_id: Uuid,
    target: i16,
    category_id: Option<Uuid>,
    stage_id: Option<Uuid>,
    title: String,
    assign: String,
    user_ids: Vec<Uuid>,
    next_user: i32,
    skip_open: bool,
    repeat: String,
    next_run_at: Option<DateTime<Utc>>,
    last_run_at: Option<DateTime<Utc>>,
    last_result: Option<Value>,
    is_active: bool,
    created_by: Option<Uuid>,
    created_at: DateTime<Utc>,
}

const COLS: &str = "id, name, segment_id, target, category_id, stage_id, title, assign, user_ids, next_user, skip_open, repeat, next_run_at, last_run_at, last_result, is_active, created_by, created_at";

async fn load(state: &AppState, id: Uuid) -> AppResult<Generator> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {COLS} FROM mkt_generators WHERE id = $1"))).bind(id).fetch_optional(&state.db).await?.ok_or(AppError::NotFound("generator"))
}

async fn describe(state: &AppState, g: &Generator) -> AppResult<Value> {
    let (made, segment): (i64, Option<String>) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM mkt_generator_items WHERE generator_id = $1), (SELECT name FROM mkt_segments WHERE id = $2)",
    )
    .bind(g.id)
    .bind(g.segment_id)
    .fetch_one(&state.db)
    .await?;
    let mut v = serde_json::to_value(g).unwrap_or_default();
    v["made"] = json!(made);
    v["segment_name"] = json!(segment);
    v["target_code"] = json!(if g.target == DEAL { "deal" } else { "lead" });
    Ok(v)
}

async fn list(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    require(&auth)?;
    let gs: Vec<Generator> = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {COLS} FROM mkt_generators ORDER BY created_at DESC"))).fetch_all(&state.db).await?;
    let mut out = Vec::new();
    for g in &gs {
        out.push(describe(&state, g).await?);
    }
    Ok(Json(json!({ "items": out })))
}

async fn get_one(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    require(&auth)?;
    let g = load(&state, id).await?;
    let mut v = describe(&state, &g).await?;
    let recent: Vec<(Uuid, Uuid, String, i64, String, DateTime<Utc>)> = sqlx::query_as(
        "SELECT g.item_id, g.client_id, i.title, i.number, c.title, g.created_at FROM mkt_generator_items g
         JOIN crm_items i ON i.id = g.item_id JOIN crm_items c ON c.id = g.client_id
         WHERE g.generator_id = $1 ORDER BY g.created_at DESC LIMIT 50",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    v["recent"] = json!(recent.into_iter().map(|(i, c, t, n, ct, at)| json!({ "item_id": i, "client_id": c, "title": t, "number": n, "client": ct, "created_at": at })).collect::<Vec<_>>());
    Ok(Json(v))
}

#[derive(Deserialize)]
struct Body {
    name: Option<String>,
    segment_id: Option<Uuid>,
    /// deal or lead.
    target: Option<String>,
    category_id: Option<Uuid>,
    stage_id: Option<Uuid>,
    title: Option<String>,
    assign: Option<String>,
    user_ids: Option<Vec<Uuid>>,
    skip_open: Option<bool>,
    repeat: Option<String>,
    /// When to run first (default: now, for a new generator).
    start_at: Option<DateTime<Utc>>,
    is_active: Option<bool>,
}

/// Checks a generator's settings as they would be after the change.
async fn check(state: &AppState, g: &Generator) -> AppResult<()> {
    let seg = load_segment(state, g.segment_id).await?;
    if ![CONTACT, COMPANY].contains(&seg.entity_type) {
        return Err(AppError::bad("the sales generator works on segments of contacts or companies"));
    }
    if !["owner", "users"].contains(&g.assign.as_str()) {
        return Err(AppError::bad("assign to the client's responsible person (owner) or chosen people (users)"));
    }
    if g.assign == "users" {
        if g.user_ids.is_empty() || g.user_ids.len() > 100 {
            return Err(AppError::bad("choose who gets the records"));
        }
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE id = ANY($1) AND is_active AND NOT is_guest").bind(&g.user_ids).fetch_one(&state.db).await?;
        if n as usize != g.user_ids.len() {
            return Err(AppError::bad("choose active employees"));
        }
    }
    if !["once", "weekly", "monthly", "quarterly"].contains(&g.repeat.as_str()) {
        return Err(AppError::bad("repeat must be once, weekly, monthly or quarterly"));
    }
    if g.target == DEAL || g.target == LEAD {
        if let Some(c) = g.category_id {
            let ok: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM crm_categories WHERE id = $1 AND entity_type = $2)").bind(c).bind(g.target).fetch_one(&state.db).await?;
            if !ok {
                return Err(AppError::bad("that pipeline is for another kind of record"));
            }
        }
        if let Some(s) = g.stage_id {
            let cat: Option<Uuid> = sqlx::query_scalar("SELECT s.category_id FROM crm_stages s JOIN crm_categories c ON c.id = s.category_id WHERE s.id = $1 AND c.entity_type = $2 AND s.semantics = 'process'")
                .bind(s)
                .bind(g.target)
                .fetch_optional(&state.db)
                .await?;
            match cat {
                None => return Err(AppError::bad("choose an open stage of that kind of record")),
                Some(c) if g.category_id.is_some_and(|x| x != c) => return Err(AppError::bad("that stage is in another pipeline")),
                _ => {}
            }
        }
    }
    Ok(())
}

fn target_of(t: &str) -> AppResult<i16> {
    match t {
        "deal" => Ok(DEAL),
        "lead" => Ok(LEAD),
        _ => Err(AppError::bad("make deals or leads")),
    }
}

async fn create(State(state): State<AppState>, auth: AuthUser, Json(b): Json<Body>) -> AppResult<Json<Value>> {
    require(&auth)?;
    let mut g = Generator {
        id: Uuid::new_v4(),
        name: required("name", b.name.as_deref().unwrap_or(""), 200)?,
        segment_id: b.segment_id.ok_or_else(|| AppError::bad("choose a segment"))?,
        target: target_of(b.target.as_deref().unwrap_or("deal"))?,
        category_id: b.category_id,
        stage_id: b.stage_id,
        title: required("title", b.title.as_deref().unwrap_or("{{name}}: repeat sale"), 300)?,
        assign: b.assign.unwrap_or_else(|| "owner".into()),
        user_ids: b.user_ids.unwrap_or_default(),
        next_user: 0,
        skip_open: b.skip_open.unwrap_or(true),
        repeat: b.repeat.unwrap_or_else(|| "once".into()),
        next_run_at: None,
        last_run_at: None,
        last_result: None,
        is_active: b.is_active.unwrap_or(true),
        created_by: Some(auth.id),
        created_at: Utc::now(),
    };
    check(&state, &g).await?;
    g.next_run_at = g.is_active.then(|| b.start_at.unwrap_or_else(Utc::now));
    sqlx::query(
        "INSERT INTO mkt_generators (id, name, segment_id, target, category_id, stage_id, title, assign, user_ids, skip_open, repeat, next_run_at, is_active, created_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)",
    )
    .bind(g.id)
    .bind(&g.name)
    .bind(g.segment_id)
    .bind(g.target)
    .bind(g.category_id)
    .bind(g.stage_id)
    .bind(&g.title)
    .bind(&g.assign)
    .bind(&g.user_ids)
    .bind(g.skip_open)
    .bind(&g.repeat)
    .bind(g.next_run_at)
    .bind(g.is_active)
    .bind(auth.id)
    .execute(&state.db)
    .await?;
    state.job_wakeup.notify_one();
    Ok(Json(describe(&state, &load(&state, g.id).await?).await?))
}

async fn update(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<Body>) -> AppResult<Json<Value>> {
    require(&auth)?;
    let mut g = load(&state, id).await?;
    if let Some(n) = &b.name {
        g.name = required("name", n, 200)?;
    }
    if let Some(s) = b.segment_id {
        g.segment_id = s;
    }
    if let Some(t) = &b.target {
        g.target = target_of(t)?;
    }
    if b.category_id.is_some() || b.target.is_some() {
        g.category_id = b.category_id;
    }
    if b.stage_id.is_some() || b.target.is_some() {
        g.stage_id = b.stage_id;
    }
    if let Some(t) = &b.title {
        g.title = required("title", t, 300)?;
    }
    if let Some(a) = b.assign {
        g.assign = a;
    }
    if let Some(u) = b.user_ids {
        g.user_ids = u;
    }
    if let Some(s) = b.skip_open {
        g.skip_open = s;
    }
    if let Some(r) = b.repeat {
        g.repeat = r;
    }
    if let Some(a) = b.is_active {
        g.is_active = a;
    }
    check(&state, &g).await?;
    let next = match (g.is_active, b.start_at) {
        (false, _) => None,
        (true, Some(at)) => Some(at),
        (true, None) => g.next_run_at.or_else(|| (g.repeat != "once" && g.last_run_at.is_some()).then(|| next_after(&g.repeat, Utc::now())).flatten()),
    };
    sqlx::query(
        "UPDATE mkt_generators SET name = $2, segment_id = $3, target = $4, category_id = $5, stage_id = $6, title = $7, assign = $8, user_ids = $9,
             skip_open = $10, repeat = $11, is_active = $12, next_run_at = $13, updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(&g.name)
    .bind(g.segment_id)
    .bind(g.target)
    .bind(g.category_id)
    .bind(g.stage_id)
    .bind(&g.title)
    .bind(&g.assign)
    .bind(&g.user_ids)
    .bind(g.skip_open)
    .bind(&g.repeat)
    .bind(g.is_active)
    .bind(next)
    .execute(&state.db)
    .await?;
    state.job_wakeup.notify_one();
    Ok(Json(describe(&state, &load(&state, id).await?).await?))
}

async fn remove(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    require(&auth)?;
    let n = sqlx::query("DELETE FROM mkt_generators WHERE id = $1").bind(id).execute(&state.db).await?.rows_affected();
    if n == 0 {
        return Err(AppError::NotFound("generator"));
    }
    Ok(Json(json!({ "ok": true })))
}

/// Runs it now (in the background), whatever its schedule.
async fn run_now(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    require(&auth)?;
    let g = load(&state, id).await?;
    jobs::enqueue(&state.db, GENERATOR_JOB, json!({ "id": g.id, "manual": true }), None).await?;
    state.job_wakeup.notify_one();
    Ok(Json(json!({ "queued": true })))
}

#[derive(FromRow)]
struct Client {
    id: Uuid,
    title: String,
    assigned_to: Option<Uuid>,
    entity_type: i16,
    /// An open record of the generator's kind already exists for this client.
    has_open: bool,
}

async fn clients(state: &AppState, g: &Generator) -> AppResult<Vec<Client>> {
    let seg = load_segment(state, g.segment_id).await?;
    let t = segment_type(state, seg.entity_type).await?;
    let filter = parse_filter(&seg.filter)?;
    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new("SELECT i.id, i.title, i.assigned_to, i.entity_type, EXISTS (SELECT 1 FROM crm_items d WHERE d.entity_type = ");
    qb.push_bind(g.target).push(" AND d.deleted_at IS NULL AND NOT d.is_closed");
    if let Some(c) = g.category_id {
        qb.push(" AND d.category_id = ").push_bind(c);
    }
    qb.push(" AND (d.company_id = i.id OR EXISTS (SELECT 1 FROM crm_item_contacts l WHERE l.item_id = d.id AND l.contact_id = i.id))) AS has_open FROM crm_items i WHERE i.deleted_at IS NULL");
    restrict(&mut qb, &t, &filter, seg.item_ids.as_deref())?;
    qb.push(" ORDER BY i.created_at LIMIT ").push_bind(MAX_CLIENTS);
    Ok(qb.build_query_as().fetch_all(&state.db).await?)
}

/// How many records a run would make now, and how many clients it would skip.
async fn preview(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    require(&auth)?;
    let g = load(&state, id).await?;
    let cs = clients(&state, &g).await?;
    let skipped = if g.skip_open { cs.iter().filter(|c| c.has_open).count() } else { 0 };
    Ok(Json(json!({ "clients": cs.len(), "would_make": cs.len() - skipped, "would_skip": skipped })))
}

fn next_after(repeat: &str, from: DateTime<Utc>) -> Option<DateTime<Utc>> {
    match repeat {
        "weekly" => Some(from + Duration::weeks(1)),
        "monthly" => from.checked_add_months(Months::new(1)),
        "quarterly" => from.checked_add_months(Months::new(3)),
        _ => None,
    }
}

fn fill(template: &str, name: &str, now: DateTime<Utc>) -> String {
    template
        .replace("{{name}}", name)
        .replace("{{ name }}", name)
        .replace("{{month}}", &now.format("%B").to_string())
        .replace("{{year}}", &now.year().to_string())
        .chars()
        .take(300)
        .collect()
}

/// Generators due: each is claimed (so one run happens) and queued.
pub async fn run_due(state: &AppState) -> anyhow::Result<()> {
    let due: Vec<Uuid> = sqlx::query_scalar(
        "UPDATE mkt_generators SET next_run_at = NULL WHERE id IN (SELECT id FROM mkt_generators WHERE is_active AND next_run_at <= now() ORDER BY next_run_at LIMIT 10 FOR UPDATE SKIP LOCKED)
         RETURNING id",
    )
    .fetch_all(&state.db)
    .await?;
    for id in due {
        jobs::enqueue(&state.db, GENERATOR_JOB, json!({ "id": id }), None).await?;
    }
    Ok(())
}

/// Background job: one run of a generator.
pub async fn run_job(state: &AppState, payload: &Value) -> anyhow::Result<()> {
    let id: Uuid = serde_json::from_value(payload["id"].clone())?;
    let manual = payload["manual"].as_bool().unwrap_or(false);
    let Ok(g) = load(state, id).await else { return Ok(()) };
    let t = entity_type_by_id(state, g.target).await?;
    let auth = system_auth();
    let now = Utc::now();
    let (mut created, mut skipped, mut failed) = (0i64, 0i64, 0i64);
    let mut turn = g.next_user.max(0) as usize;
    let cs = match clients(state, &g).await {
        Ok(c) => c,
        Err(e) => {
            sqlx::query("UPDATE mkt_generators SET last_run_at = now(), last_result = $2 WHERE id = $1").bind(id).bind(json!({ "error": e.to_string() })).execute(&state.db).await?;
            return Ok(());
        }
    };
    for c in cs {
        if g.skip_open && c.has_open {
            skipped += 1;
            continue;
        }
        let assigned = if g.assign == "users" && !g.user_ids.is_empty() {
            let u = g.user_ids[turn % g.user_ids.len()];
            turn += 1;
            Some(u)
        } else {
            c.assigned_to.or(g.created_by)
        };
        let mut body = Map::new();
        body.insert("title".into(), json!(fill(&g.title, &c.title, now)));
        if let Some(a) = assigned {
            body.insert("assigned_to".into(), json!(a));
        }
        if let Some(s) = g.stage_id {
            body.insert("stage_id".into(), json!(s));
        } else if let Some(cat) = g.category_id {
            body.insert("category_id".into(), json!(cat));
        }
        if c.entity_type == COMPANY {
            body.insert("company_id".into(), json!(c.id));
        } else {
            body.insert("contact_ids".into(), json!([c.id]));
        }
        body.insert("source_details".into(), json!(format!("Sales generator: {}", g.name)));
        match create_item(state, &auth, &t, body, Some(c.id)).await {
            Ok(item) => {
                created += 1;
                sqlx::query("INSERT INTO mkt_generator_items (generator_id, client_id, item_id) VALUES ($1, $2, $3)").bind(id).bind(c.id).bind(item).execute(&state.db).await?;
                crate::crm::broadcast(state, t.id, item, false).await;
            }
            Err(e) => {
                failed += 1;
                tracing::warn!(generator = %id, client = %c.id, error = ?e, "the sales generator could not make a record");
            }
        }
    }
    // A scheduled run sets the next one; running by hand leaves the schedule as it was.
    let next = if manual { None } else { next_after(&g.repeat, now) };
    sqlx::query(
        "UPDATE mkt_generators SET last_run_at = now(), last_result = $2, next_user = $3,
             next_run_at = CASE WHEN $5 THEN next_run_at ELSE $4 END WHERE id = $1",
    )
    .bind(id)
    .bind(json!({ "created": created, "skipped": skipped, "failed": failed, "manual": manual }))
    .bind((turn % g.user_ids.len().max(1)) as i32)
    .bind(next)
    .bind(manual)
    .execute(&state.db)
    .await?;
    if let Some(by) = g.created_by {
        crate::notify::notify(
            state,
            &[by],
            crate::notify::NewNotification {
                kind: "marketing.generator",
                title: format!("Sales generator “{}”: {created} {} made, {skipped} skipped", g.name, t.name_plural.to_lowercase()),
                body: (failed > 0).then(|| format!("{failed} could not be made")),
                link: Some("/marketing/generators".into()),
                actor: None,
            },
        )
        .await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_and_repeats() {
        let at = DateTime::parse_from_rfc3339("2026-01-31T09:00:00Z").unwrap().with_timezone(&Utc);
        assert_eq!(fill("{{name}}: renewal {{month}} {{year}}", "Acme", at), "Acme: renewal January 2026");
        assert_eq!(next_after("monthly", at).unwrap().to_rfc3339(), "2026-02-28T09:00:00+00:00");
        assert_eq!(next_after("quarterly", at).unwrap().to_rfc3339(), "2026-04-30T09:00:00+00:00");
        assert!(next_after("once", at).is_none());
    }
}
