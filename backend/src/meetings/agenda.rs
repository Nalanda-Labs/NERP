//! The agenda: topics (with sub-points) and who presents them, the reports they write, the
//! decisions recorded at the meeting, tasks made from topics, and a topic's history across
//! the meetings of a series.

use axum::{
    extract::{Path, State},
    routing::{delete, get, patch, post, put},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use super::{get_meeting, link, load, not_closed, visible};
use crate::{
    auth::AuthUser,
    error::{AppError, AppResult},
    notify::{notify, NewNotification},
    routes::{clean, required},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/meetings/{id}/agenda", post(add))
        .route("/meetings/{id}/agenda/order", put(order))
        .route("/meetings/agenda/{id}", patch(change).delete(remove))
        .route("/meetings/agenda/{id}/task", post(make_task))
        .route("/meetings/agenda/{id}/report", put(write_report).delete(drop_report))
        .route("/meetings/topics/{id}/history", get(history))
        .route("/meetings/agenda/{id}/report/{user}", delete(drop_report_of))
}

#[derive(sqlx::FromRow, Clone)]
struct Entry {
    id: Uuid,
    meeting_id: Uuid,
    topic_id: Uuid,
    parent_id: Option<Uuid>,
    sort: i32,
    description: Option<String>,
    responsible_ids: Vec<Uuid>,
    duration_minutes: Option<i32>,
    decision: Option<String>,
    status: String,
    task_id: Option<Uuid>,
    proposed_by: Option<Uuid>,
    title: String,
}

const ENTRY: &str = "SELECT a.id, a.meeting_id, a.topic_id, a.parent_id, a.sort, a.description, a.responsible_ids, a.duration_minutes, a.decision,
    a.status, a.task_id, a.proposed_by, t.title FROM meeting_agenda a JOIN meeting_topics t ON t.id = a.topic_id";

async fn entry(state: &AppState, id: Uuid) -> AppResult<Entry> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{ENTRY} WHERE a.id = $1")))
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound("agenda item"))
}

/// The agenda of a meeting as a tree, with reports, tasks and how often the topic came up before.
pub(super) async fn agenda_json(state: &AppState, meeting: Uuid) -> AppResult<Value> {
    let entries: Vec<Entry> = sqlx::query_as(sqlx::AssertSqlSafe(format!("{ENTRY} WHERE a.meeting_id = $1 ORDER BY a.sort, a.created_at")))
        .bind(meeting)
        .fetch_all(&state.db)
        .await?;
    let ids: Vec<Uuid> = entries.iter().map(|e| e.id).collect();
    let reports: Vec<(Uuid, Uuid, String, String, Vec<Uuid>, DateTime<Utc>)> = sqlx::query!(r#"SELECT r.agenda_id AS "agenda_id!", r.user_id AS "user_id!", u.first_name || ' ' || u.last_name AS "c2!", r.body AS "body!", r.file_ids AS "file_ids!", r.updated_at AS "updated_at!"
         FROM meeting_reports r JOIN users u ON u.id = r.user_id WHERE r.agenda_id = ANY($1) ORDER BY r.created_at"#, &ids).map(|r| (r.agenda_id, r.user_id, r.c2, r.body, r.file_ids, r.updated_at))
    .fetch_all(&state.db)
    .await?;
    let people: Vec<Uuid> = entries.iter().flat_map(|e| e.responsible_ids.iter().copied().chain(e.proposed_by)).collect();
    let names = super::names(state, &people).await?;
    let name_of = |u: &Uuid| names.iter().find(|n| n["id"] == json!(u)).cloned().unwrap_or(json!({ "id": u, "name": "" }));
    let mut out = Vec::new();
    for e in &entries {
        let task: Option<(Uuid, i64, String, String)> = match e.task_id {
            Some(t) => sqlx::query_as("SELECT id, number, title, status FROM tasks WHERE id = $1").bind(t).fetch_optional(&state.db).await?,
            None => None,
        };
        let earlier: i64 = sqlx::query_scalar!(r#"SELECT count(*) AS "v!" FROM meeting_agenda WHERE topic_id = $1 AND id <> $2"#, e.topic_id, e.id).fetch_one(&state.db).await?;
        let mut rs = Vec::new();
        for (_, uid, uname, body, file_ids, at) in reports.iter().filter(|r| r.0 == e.id) {
            let files: Vec<(Uuid, String, i64)> = sqlx::query!(r#"SELECT id, original_name, size_bytes FROM files WHERE id = ANY($1)"#, file_ids).map(|r| (r.id, r.original_name, r.size_bytes)).fetch_all(&state.db).await?;
            rs.push(json!({ "user_id": uid, "name": uname, "body": body, "updated_at": at,
                            "files": files.into_iter().map(|(id, n, s)| json!({ "id": id, "name": n, "size": s })).collect::<Vec<_>>() }));
        }
        out.push(json!({
            "id": e.id, "topic_id": e.topic_id, "parent_id": e.parent_id, "sort": e.sort, "title": e.title, "description": e.description,
            "responsible": e.responsible_ids.iter().map(name_of).collect::<Vec<_>>(), "duration_minutes": e.duration_minutes,
            "decision": e.decision, "status": e.status, "proposed_by": e.proposed_by.map(|u| name_of(&u)),
            "task": task.map(|(id, n, t, s)| json!({ "id": id, "number": n, "title": t, "status": s })),
            "earlier": earlier, "reports": rs,
        }));
    }
    Ok(Value::Array(out))
}

/// Checks that the people are in the meeting.
async fn check_responsible(state: &AppState, meeting: Uuid, ids: &[Uuid]) -> AppResult<Vec<Uuid>> {
    if ids.len() > 20 {
        return Err(AppError::bad("at most 20 people per topic"));
    }
    let mut out: Vec<Uuid> = Vec::new();
    for u in ids {
        if out.contains(u) {
            continue;
        }
        let ok: bool = sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM meeting_members WHERE meeting_id = $1 AND user_id = $2) AS "v!""#, meeting, u).fetch_one(&state.db).await?;
        if !ok {
            return Err(AppError::bad("the people presenting a topic take part in the meeting"));
        }
        out.push(*u);
    }
    Ok(out)
}

async fn tell_presenters(state: &AppState, meeting: Uuid, title: &str, people: &[Uuid], actor: Uuid) {
    let Ok(m) = load(state, meeting).await else { return };
    notify(state, people, NewNotification {
        kind: "meeting.topic",
        title: format!("You present “{title}” at “{}”, {}", m.title, super::when(&m)),
        body: Some("Write your report on the topic before the meeting.".into()),
        link: Some(link(meeting)),
        actor: Some(actor),
    })
    .await;
}

#[derive(Deserialize)]
struct EntryInput {
    title: Option<String>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    description: Option<Option<String>>,
    responsible_ids: Option<Vec<Uuid>>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    duration_minutes: Option<Option<i32>>,
    parent_id: Option<Uuid>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    decision: Option<Option<String>>,
    status: Option<String>,
}

fn check_minutes(d: Option<i32>) -> AppResult<Option<i32>> {
    match d {
        Some(x) if !(1..=600).contains(&x) => Err(AppError::bad("a topic takes 1 to 600 minutes")),
        _ => Ok(d),
    }
}

async fn add(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<EntryInput>) -> AppResult<Json<Value>> {
    let (m, p) = visible(&state, &auth, id).await?;
    if !p.takes_part() {
        return Err(AppError::Forbidden);
    }
    not_closed(&m)?;
    let title = required("title", b.title.as_deref().unwrap_or(""), 300)?;
    let description = clean(b.description.flatten());
    if description.as_ref().is_some_and(|d| d.chars().count() > 20_000) {
        return Err(AppError::bad("the description is too long"));
    }
    let responsible = check_responsible(&state, id, &b.responsible_ids.unwrap_or_default()).await?;
    let duration = check_minutes(b.duration_minutes.flatten())?;
    if let Some(parent) = b.parent_id {
        let pe = entry(&state, parent).await?;
        if pe.meeting_id != id || pe.parent_id.is_some() {
            return Err(AppError::bad("sub-points go under a topic of this meeting (one level deep)"));
        }
    }
    let count: i64 = sqlx::query_scalar!(r#"SELECT count(*) AS "v!" FROM meeting_agenda WHERE meeting_id = $1"#, id).fetch_one(&state.db).await?;
    if count >= 300 {
        return Err(AppError::bad("at most 300 agenda items"));
    }
    let sort: i32 = sqlx::query_scalar!(r#"SELECT max(sort) AS "v?" FROM meeting_agenda WHERE meeting_id = $1"#, id).fetch_one(&state.db).await?.unwrap_or(0) + 10;
    let mut tx = state.db.begin().await?;
    let topic: Uuid = sqlx::query_scalar!("INSERT INTO meeting_topics (title, created_by) VALUES ($1, $2) RETURNING id", &title, auth.id).fetch_one(&mut *tx).await?;
    sqlx::query!("INSERT INTO meeting_agenda (meeting_id, topic_id, parent_id, sort, description, responsible_ids, duration_minutes, proposed_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)", id, topic, b.parent_id, sort, description.as_deref(), &responsible, duration, auth.id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    tell_presenters(&state, id, &title, &responsible, auth.id).await;
    get_meeting(State(state), auth, Path(id)).await
}

async fn change(State(state): State<AppState>, auth: AuthUser, Path(aid): Path<Uuid>, Json(b): Json<EntryInput>) -> AppResult<Json<Value>> {
    let e = entry(&state, aid).await?;
    let (m, p) = visible(&state, &auth, e.meeting_id).await?;
    not_closed(&m)?;
    // People running the meeting change anything; whoever proposed a topic may reword it
    // while the agenda is being prepared.
    let own = e.proposed_by == Some(auth.id) && m.state == "prepare" && p.takes_part();
    let records = b.decision.is_some() || b.status.is_some() || b.responsible_ids.is_some();
    if !(p.runs() || (own && !records)) {
        return Err(AppError::Forbidden);
    }
    if let Some(t) = &b.title {
        let t = required("title", t, 300)?;
        sqlx::query!("UPDATE meeting_topics SET title = $2 WHERE id = $1", e.topic_id, t).execute(&state.db).await?;
    }
    let status = match b.status.as_deref() {
        Some(s) if ["open", "done"].contains(&s) => Some(s.to_string()),
        Some(_) => return Err(AppError::bad("a topic is open or done")),
        None => None,
    };
    let decision = b.decision.map(clean);
    if decision.as_ref().and_then(|d| d.as_ref()).is_some_and(|d| d.chars().count() > 20_000) {
        return Err(AppError::bad("the decision is too long"));
    }
    let responsible = match b.responsible_ids {
        Some(r) => Some(check_responsible(&state, e.meeting_id, &r).await?),
        None => None,
    };
    let duration = b.duration_minutes.map(check_minutes).transpose()?;
    sqlx::query!("UPDATE meeting_agenda SET description = CASE WHEN $2 THEN $3 ELSE description END, responsible_ids = COALESCE($4, responsible_ids),
             duration_minutes = CASE WHEN $5 THEN $6 ELSE duration_minutes END, decision = CASE WHEN $7 THEN $8 ELSE decision END,
             status = COALESCE($9, status)
         WHERE id = $1", aid, b.description.is_some(), clean(b.description.flatten()), responsible.as_deref(), duration.is_some(), duration.flatten(), decision.is_some(), decision.flatten(), status)
    .execute(&state.db)
    .await?;
    if let Some(r) = responsible {
        let added: Vec<Uuid> = r.into_iter().filter(|u| !e.responsible_ids.contains(u) && *u != auth.id).collect();
        let title = entry(&state, aid).await?.title;
        tell_presenters(&state, e.meeting_id, &title, &added, auth.id).await;
    }
    get_meeting(State(state), auth, Path(e.meeting_id)).await
}

async fn remove(State(state): State<AppState>, auth: AuthUser, Path(aid): Path<Uuid>) -> AppResult<Json<Value>> {
    let e = entry(&state, aid).await?;
    let (m, p) = visible(&state, &auth, e.meeting_id).await?;
    not_closed(&m)?;
    let own = e.proposed_by == Some(auth.id) && m.state == "prepare";
    if !(p.runs() || own) {
        return Err(AppError::Forbidden);
    }
    sqlx::query!("DELETE FROM meeting_agenda WHERE id = $1", aid).execute(&state.db).await?;
    // A topic that is on no agenda any more is gone.
    sqlx::query!("DELETE FROM meeting_topics t WHERE t.id = $1 AND NOT EXISTS (SELECT 1 FROM meeting_agenda a WHERE a.topic_id = t.id)", e.topic_id).execute(&state.db).await?;
    get_meeting(State(state), auth, Path(e.meeting_id)).await
}

#[derive(Deserialize)]
struct OrderInput {
    ids: Vec<Uuid>,
}

async fn order(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<OrderInput>) -> AppResult<Json<Value>> {
    let (m, p) = visible(&state, &auth, id).await?;
    if !p.runs() {
        return Err(AppError::Forbidden);
    }
    not_closed(&m)?;
    let mut tx = state.db.begin().await?;
    for (i, aid) in b.ids.iter().enumerate() {
        sqlx::query!("UPDATE meeting_agenda SET sort = $3 WHERE id = $1 AND meeting_id = $2", aid, id, (i as i32 + 1) * 10).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    get_meeting(State(state), auth, Path(id)).await
}

#[derive(Deserialize)]
struct TaskInput {
    title: Option<String>,
    responsible_id: Option<Uuid>,
    deadline: Option<DateTime<Utc>>,
}

/// A task from a topic (an action item), in the meeting's workgroup when the person making it
/// works there.
async fn make_task(State(state): State<AppState>, auth: AuthUser, Path(aid): Path<Uuid>, Json(b): Json<TaskInput>) -> AppResult<Json<Value>> {
    let e = entry(&state, aid).await?;
    let (m, p) = visible(&state, &auth, e.meeting_id).await?;
    if !p.runs() {
        return Err(AppError::Forbidden);
    }
    if e.task_id.is_some() {
        return Err(AppError::Conflict("this topic already has a task".into()));
    }
    let title = match b.title {
        Some(t) => required("title", &t, 300)?,
        None => e.title.clone(),
    };
    let mut description = format!("From the meeting “{}” ({}{}).", m.title, state.config.public_url, link(m.id));
    if let Some(d) = &e.decision {
        description.push_str(&format!("\n\nDecision: {d}"));
    }
    // In the meeting's workgroup when the person making it works there (the keeper may not).
    let project = match m.project_id {
        Some(p) => sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM project_members WHERE project_id = $1 AND user_id = $2 AND role <> 'viewer') AS "v!""#, p, auth.id)
            .fetch_one(&state.db)
            .await?
            .then_some(p),
        None => None,
    };
    let task = crate::tasks::items::create_task(&state, &auth, crate::tasks::items::CreateTask {
        title,
        description: Some(description),
        responsible_id: b.responsible_id.or(e.responsible_ids.first().copied()),
        deadline: b.deadline,
        project_id: project,
        ..Default::default()
    })
    .await?;
    sqlx::query!("UPDATE meeting_agenda SET task_id = $2 WHERE id = $1", aid, task).execute(&state.db).await?;
    get_meeting(State(state), auth, Path(e.meeting_id)).await
}

#[derive(Deserialize)]
struct ReportInput {
    body: String,
    #[serde(default)]
    file_ids: Vec<Uuid>,
}

/// A presenter's report on their topic (one per person; writing again replaces it).
async fn write_report(State(state): State<AppState>, auth: AuthUser, Path(aid): Path<Uuid>, Json(b): Json<ReportInput>) -> AppResult<Json<Value>> {
    let e = entry(&state, aid).await?;
    let (m, p) = visible(&state, &auth, e.meeting_id).await?;
    not_closed(&m)?;
    if !(e.responsible_ids.contains(&auth.id) || p.runs()) || !p.takes_part() {
        return Err(AppError::bad("reports are written by the people presenting the topic"));
    }
    let body = required("report", &b.body, 50_000)?;
    if b.file_ids.len() > 20 {
        return Err(AppError::bad("at most 20 files"));
    }
    let current: Vec<Uuid> = sqlx::query_scalar!("SELECT file_ids FROM meeting_reports WHERE agenda_id = $1 AND user_id = $2", aid, auth.id)
        .fetch_optional(&state.db)
        .await?
        .unwrap_or_default();
    super::own_files(&state, &auth, &b.file_ids, &current).await?;
    sqlx::query!("INSERT INTO meeting_reports (agenda_id, user_id, body, file_ids) VALUES ($1, $2, $3, $4)
         ON CONFLICT (agenda_id, user_id) DO UPDATE SET body = EXCLUDED.body, file_ids = EXCLUDED.file_ids, updated_at = now()", aid, auth.id, &body, &b.file_ids)
    .execute(&state.db)
    .await?;
    get_meeting(State(state), auth, Path(e.meeting_id)).await
}

async fn drop_report(State(state): State<AppState>, auth: AuthUser, Path(aid): Path<Uuid>) -> AppResult<Json<Value>> {
    let e = entry(&state, aid).await?;
    let (m, _) = visible(&state, &auth, e.meeting_id).await?;
    not_closed(&m)?;
    sqlx::query!("DELETE FROM meeting_reports WHERE agenda_id = $1 AND user_id = $2", aid, auth.id).execute(&state.db).await?;
    get_meeting(State(state), auth, Path(e.meeting_id)).await
}

/// People running the meeting may remove someone's report (e.g. posted on the wrong topic).
async fn drop_report_of(State(state): State<AppState>, auth: AuthUser, Path((aid, user)): Path<(Uuid, Uuid)>) -> AppResult<Json<Value>> {
    let e = entry(&state, aid).await?;
    let (m, p) = visible(&state, &auth, e.meeting_id).await?;
    if !p.runs() {
        return Err(AppError::Forbidden);
    }
    not_closed(&m)?;
    sqlx::query!("DELETE FROM meeting_reports WHERE agenda_id = $1 AND user_id = $2", aid, user).execute(&state.db).await?;
    get_meeting(State(state), auth, Path(e.meeting_id)).await
}

/// Every meeting a topic came up in (that this person may see): reports, decisions, status.
async fn history(State(state): State<AppState>, auth: AuthUser, Path(topic): Path<Uuid>) -> AppResult<Json<Value>> {
    let rows: Vec<(Uuid, Uuid, String, DateTime<Utc>, String, Option<String>, String)> = sqlx::query!(r#"SELECT a.id, m.id AS "c1", m.title, m.starts_at, m.state, a.decision, a.status FROM meeting_agenda a JOIN meetings m ON m.id = a.meeting_id
         WHERE a.topic_id = $1 ORDER BY m.starts_at"#, topic).map(|r| (r.id, r.c1, r.title, r.starts_at, r.state, r.decision, r.status))
    .fetch_all(&state.db)
    .await?;
    let title: Option<String> = sqlx::query_scalar!("SELECT title FROM meeting_topics WHERE id = $1", topic).fetch_optional(&state.db).await?;
    let mut items = Vec::new();
    for (aid, mid, mtitle, starts, mstate, decision, status) in rows {
        if visible(&state, &auth, mid).await.is_err() {
            continue;
        }
        let reports: Vec<(String, String)> = sqlx::query!(r#"SELECT u.first_name || ' ' || u.last_name AS "c0!", r.body AS "body!" FROM meeting_reports r JOIN users u ON u.id = r.user_id WHERE r.agenda_id = $1 ORDER BY r.created_at"#, aid).map(|r| (r.c0, r.body))
        .fetch_all(&state.db)
        .await?;
        items.push(json!({ "meeting": { "id": mid, "title": mtitle, "starts_at": starts, "state": mstate }, "decision": decision, "status": status,
                           "reports": reports.into_iter().map(|(n, b)| json!({ "name": n, "body": b })).collect::<Vec<_>>() }));
    }
    if items.is_empty() {
        return Err(AppError::NotFound("topic"));
    }
    Ok(Json(json!({ "title": title, "items": items })))
}

/// Puts the unfinished topics of one meeting (and their unfinished sub-points) on the next
/// one's agenda; they are marked "carried" in the first.
pub(super) async fn carry_over(state: &AppState, from: Uuid, to: Uuid) -> AppResult<()> {
    let entries: Vec<Entry> = sqlx::query_as(sqlx::AssertSqlSafe(format!("{ENTRY} WHERE a.meeting_id = $1 AND a.status = 'open' ORDER BY a.parent_id NULLS FIRST, a.sort")))
        .bind(from)
        .fetch_all(&state.db)
        .await?;
    let members: Vec<Uuid> = sqlx::query_scalar!("SELECT user_id FROM meeting_members WHERE meeting_id = $1", to).fetch_all(&state.db).await?;
    let mut mapped: Vec<(Uuid, Uuid)> = Vec::new();
    let mut tx = state.db.begin().await?;
    for e in &entries {
        let parent = match e.parent_id {
            Some(p) => match mapped.iter().find(|(old, _)| *old == p) {
                Some((_, new)) => Some(*new),
                None => continue, // its topic was settled: the sub-point stays with it
            },
            None => None,
        };
        let responsible: Vec<Uuid> = e.responsible_ids.iter().copied().filter(|u| members.contains(u)).collect();
        let new: Uuid = sqlx::query_scalar!("INSERT INTO meeting_agenda (meeting_id, topic_id, parent_id, sort, description, responsible_ids, duration_minutes, proposed_by)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING id", to, e.topic_id, parent, e.sort, e.description.as_deref(), &responsible, e.duration_minutes, e.proposed_by)
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query!("UPDATE meeting_agenda SET status = 'carried' WHERE id = $1", e.id).execute(&mut *tx).await?;
        mapped.push((e.id, new));
    }
    tx.commit().await?;
    Ok(())
}
