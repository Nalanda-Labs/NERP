use axum::{
    extract::{Path, Query, State},
    routing::{get, post, put},
    Json, Router,
};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{FromRow, Postgres, QueryBuilder};
use uuid::Uuid;

use super::access::{project_access, ProjectAccess};
use crate::{
    audit,
    auth::{permissions as perm, AuthUser, ClientIp},
    error::{is_fk_violation, AppError, AppResult},
    notify::{notify, NewNotification},
    routes::{clean, double_option, required},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/projects", get(list).post(create))
        .route("/projects/{id}", get(detail).patch(update).delete(remove))
        .route("/projects/{id}/join", post(join))
        .route("/projects/{id}/leave", post(leave))
        .route("/projects/{id}/members/{user_id}", put(set_member).delete(remove_member))
        .route("/projects/{id}/requests", get(requests))
        .route("/projects/{id}/requests/{user_id}", post(answer_request).delete(withdraw_request))
        .route("/projects/{id}/chat", post(open_chat))
        .route("/projects/{id}/copy", post(copy))
}

const COLORS: &[&str] = &["indigo", "sky", "emerald", "amber", "rose", "violet", "teal", "orange", "slate"];

#[derive(Serialize, FromRow)]
pub struct ProjectSummary {
    id: Uuid,
    name: String,
    description: Option<String>,
    visibility: String,
    color: String,
    is_archived: bool,
    member_count: i64,
    open_task_count: i64,
    overdue_task_count: i64,
    my_role: Option<String>,
    created_at: DateTime<Utc>,
    /// group (a workgroup) | project (with dates) | scrum (a Scrum team)
    kind: String,
    starts_on: Option<NaiveDate>,
    ends_on: Option<NaiveDate>,
    /// I asked to join (closed groups).
    requested: bool,
    /// Requests waiting for the owners.
    request_count: i64,
    /// The latest change to its tasks, or post, or message.
    last_activity_at: DateTime<Utc>,
    /// Messages in its chat I haven't read.
    unread: i64,
}

#[derive(Serialize, FromRow)]
pub struct Member {
    user_id: Uuid,
    full_name: String,
    position: Option<String>,
    avatar_file_id: Option<Uuid>,
    role: String,
    is_guest: bool,
}

#[derive(Serialize)]
pub struct ProjectDetail {
    #[serde(flatten)]
    summary: ProjectSummary,
    members: Vec<Member>,
    stages: Vec<super::stages::Stage>,
    access: ProjectAccess,
}

const SUMMARY_SELECT: &str = "SELECT p.id, p.name, p.description, p.visibility, p.color, p.is_archived,
    (SELECT COUNT(*) FROM project_members pm WHERE pm.project_id = p.id) AS member_count,
    (SELECT COUNT(*) FROM tasks t WHERE t.project_id = p.id AND t.status IN ('new','in_progress','review')) AS open_task_count,
    (SELECT COUNT(*) FROM tasks t WHERE t.project_id = p.id AND t.status IN ('new','in_progress','review') AND t.deadline < now()) AS overdue_task_count,
    (SELECT role FROM project_members pm WHERE pm.project_id = p.id AND pm.user_id = ";

fn summary_query(user: Uuid) -> QueryBuilder<Postgres> {
    let mut qb = QueryBuilder::new(SUMMARY_SELECT);
    qb.push_bind(user)
        .push(") AS my_role, p.created_at, p.kind, p.starts_on, p.ends_on, EXISTS (SELECT 1 FROM project_join_requests r WHERE r.project_id = p.id AND r.user_id = ")
        .push_bind(user)
        .push(") AS requested, (SELECT COUNT(*) FROM project_join_requests r WHERE r.project_id = p.id) AS request_count,
            GREATEST(p.updated_at,
                COALESCE((SELECT MAX(t.last_activity_at) FROM tasks t WHERE t.project_id = p.id), p.updated_at),
                COALESCE((SELECT MAX(f.created_at) FROM feed_posts f WHERE f.project_id = p.id), p.updated_at),
                COALESCE((SELECT c.last_message_at FROM im_chats c WHERE c.project_id = p.id), p.updated_at)) AS last_activity_at,
            COALESCE((SELECT COUNT(*) FROM im_chats c JOIN im_members me ON me.chat_id = c.id AND me.user_id = ")
        .push_bind(user)
        .push(" JOIN im_messages m ON m.chat_id = c.id AND m.created_at > COALESCE(me.last_read_at, '-infinity') AND m.user_id IS DISTINCT FROM me.user_id AND m.deleted_at IS NULL
              WHERE c.project_id = p.id), 0) AS unread FROM projects p WHERE TRUE");
    qb
}

#[derive(Deserialize)]
struct ListQuery {
    q: Option<String>,
    /// "true" to list archived projects instead of active ones.
    archived: Option<String>,
    /// "true" to list only projects I belong to.
    mine: Option<String>,
    /// group | project
    kind: Option<String>,
    /// name (default) | activity
    sort: Option<String>,
    /// "true": only those with overdue tasks.
    overdue: Option<String>,
    /// "true": only those with messages I haven't read.
    unread: Option<String>,
}

async fn list(State(state): State<AppState>, auth: AuthUser, Query(q): Query<ListQuery>) -> AppResult<Json<Vec<ProjectSummary>>> {
    let mut qb = summary_query(auth.id);
    // Open and closed groups are listed for everyone; private ones for their members.
    if auth.is_guest() {
        // Guests: only the groups they were invited to.
        qb.push(" AND EXISTS (SELECT 1 FROM project_members pm WHERE pm.project_id = p.id AND pm.user_id = ").push_bind(auth.id).push(")");
    } else if !auth.can(perm::TASKS_MANAGE_ALL) {
        qb.push(" AND (p.visibility IN ('open', 'closed') OR EXISTS (SELECT 1 FROM project_members pm WHERE pm.project_id = p.id AND pm.user_id = ")
            .push_bind(auth.id)
            .push("))");
    }
    if let Some(k) = q.kind.as_deref().filter(|k| ["group", "project", "scrum"].contains(k)) {
        qb.push(" AND p.kind = ").push_bind(k.to_string());
    }
    qb.push(" AND p.is_archived = ").push_bind(q.archived.as_deref() == Some("true"));
    if q.mine.as_deref() == Some("true") {
        qb.push(" AND EXISTS (SELECT 1 FROM project_members pm WHERE pm.project_id = p.id AND pm.user_id = ")
            .push_bind(auth.id)
            .push(")");
    }
    if let Some(term) = clean(q.q) {
        qb.push(" AND p.name ILIKE ").push_bind(format!("%{}%", term.replace('%', "\\%").replace('_', "\\_")));
    }
    if q.overdue.as_deref() == Some("true") {
        qb.push(" AND EXISTS (SELECT 1 FROM tasks t WHERE t.project_id = p.id AND t.status IN ('new','in_progress','review') AND t.deadline < now())");
    }
    // The summary's columns can't be filtered on here; the list is short, so filter after.
    qb.push(if q.sort.as_deref() == Some("activity") { " ORDER BY last_activity_at DESC LIMIT 500" } else { " ORDER BY p.name LIMIT 500" });
    let mut rows: Vec<ProjectSummary> = qb.build_query_as().fetch_all(&state.db).await?;
    if q.unread.as_deref() == Some("true") {
        rows.retain(|p| p.unread > 0);
    }
    Ok(Json(rows))
}

async fn load_detail(state: &AppState, auth: &AuthUser, id: Uuid) -> AppResult<ProjectDetail> {
    let access = project_access(state, auth, id).await?;
    let mut qb = summary_query(auth.id);
    qb.push(" AND p.id = ").push_bind(id);
    let summary: ProjectSummary = qb.build_query_as().fetch_one(&state.db).await?;
    let members: Vec<Member> = sqlx::query_as!(Member, r#"SELECT u.id AS "user_id!", u.first_name || ' ' || u.last_name AS "full_name!", u.position AS "position?", u.avatar_file_id AS "avatar_file_id?", pm.role AS "role!", u.is_guest AS "is_guest!"
         FROM project_members pm JOIN users u ON u.id = pm.user_id
         WHERE pm.project_id = $1
         ORDER BY CASE pm.role WHEN 'owner' THEN 0 WHEN 'member' THEN 1 ELSE 2 END, u.last_name, u.first_name"#, id)
    .fetch_all(&state.db)
    .await?;
    let stages = super::stages::load_stages(state, id).await?;
    Ok(ProjectDetail { summary, members, stages, access })
}

async fn detail(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<ProjectDetail>> {
    Ok(Json(detail_or_card(&state, &auth, id).await?))
}

/// The full detail, or for someone outside a closed group just its card (name, description,
/// owners), so they can ask to join.
async fn detail_or_card(state: &AppState, auth: &AuthUser, id: Uuid) -> AppResult<ProjectDetail> {
    match load_detail(state, auth, id).await {
        Err(AppError::NotFound(_)) => {
            let mut qb = summary_query(auth.id);
            qb.push(" AND p.visibility = 'closed' AND p.id = ").push_bind(id);
            let summary: ProjectSummary = qb.build_query_as().fetch_optional(&state.db).await?.ok_or(AppError::NotFound("project"))?;
            let members: Vec<Member> = sqlx::query_as!(Member, r#"SELECT u.id AS "user_id!", u.first_name || ' ' || u.last_name AS "full_name!", u.position AS "position?", u.avatar_file_id AS "avatar_file_id?", pm.role AS "role!", u.is_guest AS "is_guest!"
                 FROM project_members pm JOIN users u ON u.id = pm.user_id WHERE pm.project_id = $1 AND pm.role = 'owner' ORDER BY u.last_name, u.first_name"#, id)
            .fetch_all(&state.db)
            .await?;
            Ok(ProjectDetail { summary, members, stages: vec![], access: ProjectAccess { view: false, manage: false, contribute: false, role: None } })
        }
        other => other,
    }
}

fn validate_visibility(v: &str) -> AppResult<()> {
    if ["open", "closed", "private"].contains(&v) { Ok(()) } else { Err(AppError::bad("visibility must be open, closed or private")) }
}

fn validate_kind(k: &str) -> AppResult<()> {
    if ["group", "project", "scrum"].contains(&k) { Ok(()) } else { Err(AppError::bad("kind must be group, project or scrum")) }
}

fn validate_dates(starts: Option<NaiveDate>, ends: Option<NaiveDate>) -> AppResult<()> {
    match (starts, ends) {
        (Some(s), Some(e)) if e < s => Err(AppError::bad("the project can't end before it starts")),
        _ => Ok(()),
    }
}

fn validate_color(c: &str) -> AppResult<()> {
    if COLORS.contains(&c) { Ok(()) } else { Err(AppError::bad("unknown colour")) }
}

#[derive(Deserialize)]
struct CreateProject {
    name: String,
    description: Option<String>,
    visibility: Option<String>,
    color: Option<String>,
    #[serde(default)]
    member_ids: Vec<Uuid>,
    kind: Option<String>,
    starts_on: Option<NaiveDate>,
    ends_on: Option<NaiveDate>,
}

async fn create(
    State(state): State<AppState>,
    auth: AuthUser,
    ClientIp(ip): ClientIp,
    Json(body): Json<CreateProject>,
) -> AppResult<Json<ProjectDetail>> {
    auth.require(perm::PROJECTS_CREATE)?;
    let name = required("name", &body.name, 200)?;
    let visibility = body.visibility.unwrap_or_else(|| "private".into());
    validate_visibility(&visibility)?;
    let color = body.color.unwrap_or_else(|| COLORS[rand::random_range(0..COLORS.len() - 1)].into());
    validate_color(&color)?;
    let kind = body.kind.unwrap_or_else(|| "project".into());
    validate_kind(&kind)?;
    validate_dates(body.starts_on, body.ends_on)?;

    let mut tx = state.db.begin().await?;
    let id: Uuid = sqlx::query_scalar!("INSERT INTO projects (name, description, visibility, color, created_by, kind, starts_on, ends_on) VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING id", &name, clean(body.description), &visibility, &color, auth.id, &kind, body.starts_on, body.ends_on)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query!("INSERT INTO project_members (project_id, user_id, role) VALUES ($1, $2, 'owner')", id, auth.id)
        .execute(&mut *tx)
        .await?;
    if kind == "scrum" {
        // Whoever sets up a Scrum team is its first Scrum master.
        sqlx::query!("UPDATE projects SET scrum_master_id = $2 WHERE id = $1", id, auth.id).execute(&mut *tx).await?;
    }
    for (i, (name, color, status)) in
        [("To do", "slate", "new"), ("In progress", "sky", "in_progress"), ("Review", "amber", "review"), ("Done", "emerald", "done")]
            .iter()
            .enumerate()
    {
        sqlx::query!("INSERT INTO project_stages (project_id, name, color, sort, status) VALUES ($1, $2, $3, $4, $5)", id, name, color, (i + 1) as f64, status)
            .execute(&mut *tx)
            .await?;
    }
    let others: Vec<Uuid> = body.member_ids.into_iter().filter(|m| *m != auth.id).collect();
    sqlx::query!("INSERT INTO project_members (project_id, user_id, role)
         SELECT $1, u.id, 'member' FROM users u WHERE u.id = ANY($2) AND u.is_active ON CONFLICT DO NOTHING", id, &others)
    .execute(&mut *tx)
    .await?;
    audit::record(&mut *tx, Some(auth.id), "project.create", "project", Some(id.to_string()), json!({ "name": name }), ip.as_deref()).await;
    tx.commit().await?;

    notify_added(&state, id, &name, &others, auth.id).await;
    Ok(Json(load_detail(&state, &auth, id).await?))
}

async fn notify_added(state: &AppState, project: Uuid, name: &str, users: &[Uuid], actor: Uuid) {
    notify(
        state,
        users,
        NewNotification {
            kind: "project.added",
            title: format!("You were added to the project {name}"),
            body: None,
            link: Some(format!("/projects/{project}")),
            actor: Some(actor),
        },
    )
    .await;
}

#[derive(Deserialize)]
struct UpdateProject {
    name: Option<String>,
    #[serde(default, deserialize_with = "double_option")]
    description: Option<Option<String>>,
    visibility: Option<String>,
    color: Option<String>,
    is_archived: Option<bool>,
    kind: Option<String>,
    #[serde(default, deserialize_with = "double_option")]
    starts_on: Option<Option<NaiveDate>>,
    #[serde(default, deserialize_with = "double_option")]
    ends_on: Option<Option<NaiveDate>>,
}

async fn update(
    State(state): State<AppState>,
    auth: AuthUser,
    ClientIp(ip): ClientIp,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateProject>,
) -> AppResult<Json<ProjectDetail>> {
    if !project_access(&state, &auth, id).await?.manage {
        return Err(AppError::Forbidden);
    }
    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new("UPDATE projects SET updated_at = now()");
    if let Some(v) = &body.name {
        qb.push(", name = ").push_bind(required("name", v, 200)?);
    }
    if let Some(v) = body.description {
        qb.push(", description = ").push_bind(clean(v));
    }
    if let Some(v) = &body.visibility {
        validate_visibility(v)?;
        qb.push(", visibility = ").push_bind(v.clone());
    }
    if let Some(v) = &body.color {
        validate_color(v)?;
        qb.push(", color = ").push_bind(v.clone());
    }
    if let Some(v) = body.is_archived {
        qb.push(", is_archived = ").push_bind(v);
    }
    if let Some(v) = &body.kind {
        validate_kind(v)?;
        qb.push(", kind = ").push_bind(v.clone());
    }
    if body.starts_on.is_some() || body.ends_on.is_some() {
        let (s0, e0): (Option<NaiveDate>, Option<NaiveDate>) = sqlx::query!(r#"SELECT starts_on, ends_on FROM projects WHERE id = $1"#, id).map(|r| (r.starts_on, r.ends_on)).fetch_one(&state.db).await?;
        let (s1, e1) = (body.starts_on.unwrap_or(s0), body.ends_on.unwrap_or(e0));
        validate_dates(s1, e1)?;
        qb.push(", starts_on = ").push_bind(s1).push(", ends_on = ").push_bind(e1);
    }
    qb.push(" WHERE id = ").push_bind(id);
    qb.build().execute(&state.db).await?;
    if body.name.is_some() {
        crate::im::sync_project_chat(&state, id).await?;
    }
    audit::record(&state.db, Some(auth.id), "project.update", "project", Some(id.to_string()), json!({}), ip.as_deref()).await;
    Ok(Json(load_detail(&state, &auth, id).await?))
}

async fn remove(
    State(state): State<AppState>,
    auth: AuthUser,
    ClientIp(ip): ClientIp,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    if !project_access(&state, &auth, id).await?.manage {
        return Err(AppError::Forbidden);
    }
    let tasks: i64 = sqlx::query_scalar!(r#"SELECT COUNT(*) AS "v!" FROM tasks WHERE project_id = $1"#, id)
        .fetch_one(&state.db)
        .await?;
    if tasks > 0 {
        return Err(AppError::bad(format!(
            "this project still has {tasks} tasks; archive it instead, or move or delete its tasks first"
        )));
    }
    // The group's drive goes with it, every version's file included.
    let files: Vec<Uuid> = sqlx::query_scalar!(r#"SELECT v.file_id AS "file_id!" FROM drive_versions v JOIN drive_nodes n ON n.id = v.node_id WHERE n.project_id = $1 AND v.file_id IS NOT NULL"#, id)
    .fetch_all(&state.db)
    .await?;
    let name: String = sqlx::query_scalar!("DELETE FROM projects WHERE id = $1 RETURNING name", id)
        .fetch_one(&state.db)
        .await?;
    for f in files {
        crate::routes::files::delete_stored(&state, f).await;
    }
    audit::record(&state.db, Some(auth.id), "project.delete", "project", Some(id.to_string()), json!({ "name": name }), ip.as_deref()).await;
    Ok(Json(json!({ "ok": true })))
}

/// Anyone may join an open group; joining a closed one sends its owners a request.
#[derive(Deserialize, Default)]
struct JoinBody {
    message: Option<String>,
}

async fn join(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, body: Option<Json<JoinBody>>) -> AppResult<Json<ProjectDetail>> {
    if auth.is_guest() {
        return Err(AppError::NotFound("project"));
    }
    let (visibility, role, name): (String, Option<String>, String) = sqlx::query!(r#"SELECT p.visibility, (SELECT role FROM project_members WHERE project_id = p.id AND user_id = $2) AS "c1", p.name FROM projects p WHERE p.id = $1"#, id, auth.id).map(|r| (r.visibility, r.c1, r.name))
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::NotFound("project"))?;
    if role.is_none() {
        match visibility.as_str() {
            "open" => {
                sqlx::query!("INSERT INTO project_members (project_id, user_id, role) VALUES ($1, $2, 'member') ON CONFLICT DO NOTHING", id, auth.id)
                    .execute(&state.db)
                    .await?;
                crate::im::sync_project_chat(&state, id).await?;
            }
            "closed" => {
                let message = body.and_then(|b| clean(b.0.message)).map(|m| m.chars().take(1000).collect::<String>());
                let new = sqlx::query!("INSERT INTO project_join_requests (project_id, user_id, message) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING", id, auth.id, message.as_deref())
                    .execute(&state.db)
                    .await?
                    .rows_affected()
                    > 0;
                if new {
                    let owners: Vec<Uuid> = sqlx::query_scalar!("SELECT user_id FROM project_members WHERE project_id = $1 AND role = 'owner'", id).fetch_all(&state.db).await?;
                    let who: String = sqlx::query_scalar!(r#"SELECT trim(first_name || ' ' || last_name) AS "v!" FROM users WHERE id = $1"#, auth.id).fetch_one(&state.db).await?;
                    notify(&state, &owners, NewNotification {
                        kind: "project.join_request",
                        title: format!("{who} asks to join {name}"),
                        body: message,
                        link: Some(format!("/projects/{id}?tab=members")),
                        actor: Some(auth.id),
                    })
                    .await;
                }
            }
            _ => return Err(AppError::NotFound("project")),
        }
    }
    Ok(Json(detail_or_card(&state, &auth, id).await?))
}

async fn requests(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    if !project_access(&state, &auth, id).await?.manage {
        return Err(AppError::Forbidden);
    }
    let rows: Vec<(Uuid, String, Option<String>, Option<Uuid>, Option<String>, DateTime<Utc>)> = sqlx::query!(r#"SELECT u.id AS "id!", trim(u.first_name || ' ' || u.last_name) AS "c1!", u.position AS "position?", u.avatar_file_id AS "avatar_file_id?", r.message AS "message?", r.created_at AS "created_at!"
         FROM project_join_requests r JOIN users u ON u.id = r.user_id WHERE r.project_id = $1 ORDER BY r.created_at"#, id).map(|r| (r.id, r.c1, r.position, r.avatar_file_id, r.message, r.created_at))
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({ "items": rows.into_iter().map(|(uid, name, pos, av, msg, at)| json!({ "user_id": uid, "full_name": name, "position": pos, "avatar_file_id": av, "message": msg, "created_at": at })).collect::<Vec<_>>() })))
}

#[derive(Deserialize)]
struct Answer {
    approve: bool,
}

async fn answer_request(State(state): State<AppState>, auth: AuthUser, Path((id, user)): Path<(Uuid, Uuid)>, Json(b): Json<Answer>) -> AppResult<Json<ProjectDetail>> {
    if !project_access(&state, &auth, id).await?.manage {
        return Err(AppError::Forbidden);
    }
    let had = sqlx::query!("DELETE FROM project_join_requests WHERE project_id = $1 AND user_id = $2", id, user).execute(&state.db).await?.rows_affected() > 0;
    if !had {
        return Err(AppError::NotFound("request"));
    }
    let name: String = sqlx::query_scalar!("SELECT name FROM projects WHERE id = $1", id).fetch_one(&state.db).await?;
    if b.approve {
        sqlx::query!("INSERT INTO project_members (project_id, user_id, role) SELECT $1, id, 'member' FROM users WHERE id = $2 AND is_active ON CONFLICT DO NOTHING", id, user)
            .execute(&state.db)
            .await?;
        crate::im::sync_project_chat(&state, id).await?;
        notify_added(&state, id, &name, &[user], auth.id).await;
    } else {
        notify(&state, &[user], NewNotification {
            kind: "project.join_declined",
            title: format!("Your request to join {name} was declined"),
            body: None,
            link: None,
            actor: Some(auth.id),
        })
        .await;
    }
    Ok(Json(load_detail(&state, &auth, id).await?))
}

async fn withdraw_request(State(state): State<AppState>, auth: AuthUser, Path((id, user)): Path<(Uuid, Uuid)>) -> AppResult<Json<Value>> {
    if user != auth.id {
        return Err(AppError::Forbidden);
    }
    sqlx::query!("DELETE FROM project_join_requests WHERE project_id = $1 AND user_id = $2", id, user).execute(&state.db).await?;
    Ok(Json(json!({ "ok": true })))
}

/// The group's chat (members only), made on first use.
async fn open_chat(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    if project_access(&state, &auth, id).await?.role.is_none() {
        return Err(AppError::bad("join the group to use its chat"));
    }
    Ok(Json(json!({ "chat_id": crate::im::project_chat(&state, id).await? })))
}

async fn leave(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let access = project_access(&state, &auth, id).await?;
    if access.role == Some("owner") {
        ensure_another_owner(&state, id, auth.id).await?;
    }
    sqlx::query!("DELETE FROM project_members WHERE project_id = $1 AND user_id = $2", id, auth.id)
        .execute(&state.db)
        .await?;
    crate::im::sync_project_chat(&state, id).await?;
    Ok(Json(json!({ "ok": true })))
}

async fn ensure_another_owner(state: &AppState, project: Uuid, leaving: Uuid) -> AppResult<()> {
    let others: i64 = sqlx::query_scalar!(r#"SELECT COUNT(*) AS "v!" FROM project_members WHERE project_id = $1 AND role = 'owner' AND user_id <> $2"#, project, leaving)
    .fetch_one(&state.db)
    .await?;
    if others == 0 {
        return Err(AppError::bad("a project needs at least one owner; make someone else an owner first"));
    }
    Ok(())
}

#[derive(Deserialize)]
struct SetMember {
    role: String,
}

async fn set_member(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((id, user)): Path<(Uuid, Uuid)>,
    Json(body): Json<SetMember>,
) -> AppResult<Json<ProjectDetail>> {
    if !project_access(&state, &auth, id).await?.manage {
        return Err(AppError::Forbidden);
    }
    if !["owner", "member", "viewer"].contains(&body.role.as_str()) {
        return Err(AppError::bad("role must be owner, member or viewer"));
    }
    let previous: Option<String> = sqlx::query_scalar!("SELECT role FROM project_members WHERE project_id = $1 AND user_id = $2", id, user)
        .fetch_optional(&state.db)
        .await?;
    if previous.as_deref() == Some("owner") && body.role != "owner" {
        ensure_another_owner(&state, id, user).await?;
    }
    sqlx::query!("INSERT INTO project_members (project_id, user_id, role) VALUES ($1, $2, $3)
         ON CONFLICT (project_id, user_id) DO UPDATE SET role = EXCLUDED.role", id, user, &body.role)
    .execute(&state.db)
    .await
    .map_err(|e| if is_fk_violation(&e) { AppError::NotFound("user") } else { e.into() })?;
    sqlx::query!("DELETE FROM project_join_requests WHERE project_id = $1 AND user_id = $2", id, user).execute(&state.db).await?;
    crate::im::sync_project_chat(&state, id).await?;
    if previous.is_none() {
        let name: String = sqlx::query_scalar!("SELECT name FROM projects WHERE id = $1", id).fetch_one(&state.db).await?;
        notify_added(&state, id, &name, &[user], auth.id).await;
    }
    Ok(Json(load_detail(&state, &auth, id).await?))
}

async fn remove_member(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((id, user)): Path<(Uuid, Uuid)>,
) -> AppResult<Json<ProjectDetail>> {
    if !project_access(&state, &auth, id).await?.manage {
        return Err(AppError::Forbidden);
    }
    let role: Option<String> = sqlx::query_scalar!("SELECT role FROM project_members WHERE project_id = $1 AND user_id = $2", id, user)
        .fetch_optional(&state.db)
        .await?;
    if role.as_deref() == Some("owner") {
        ensure_another_owner(&state, id, user).await?;
    }
    sqlx::query!("DELETE FROM project_members WHERE project_id = $1 AND user_id = $2", id, user)
        .execute(&state.db)
        .await?;
    crate::im::sync_project_chat(&state, id).await?;
    Ok(Json(load_detail(&state, &auth, id).await?))
}

// ---------------------------------------------------------------------------
// Copying a project (projects as templates)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct CopyProject {
    name: String,
    /// The copy's start; task dates move by the same amount as the project's start.
    starts_on: Option<NaiveDate>,
    #[serde(default)]
    members: bool,
    #[serde(default = "yes")]
    tasks: bool,
}
fn yes() -> bool {
    true
}

/// Makes a new project from this one: its settings, columns, custom fields and board
/// automation; optionally its members; and its tasks (as new ones, with their checklists and
/// subtasks, dates moved with the project's start).
pub async fn copy(State(state): State<AppState>, auth: AuthUser, ClientIp(ip): ClientIp, Path(id): Path<Uuid>, Json(b): Json<CopyProject>) -> AppResult<Json<ProjectDetail>> {
    auth.require(perm::PROJECTS_CREATE)?;
    project_access(&state, &auth, id).await?;
    let name = required("name", &b.name, 200)?;
    #[allow(clippy::type_complexity)]
    let (desc, vis, color, kind, s0, e0, sprint_days): (Option<String>, String, String, String, Option<NaiveDate>, Option<NaiveDate>, i32) =
        sqlx::query!(r#"SELECT description, visibility, color, kind, starts_on, ends_on, sprint_days FROM projects WHERE id = $1"#, id).map(|r| (r.description, r.visibility, r.color, r.kind, r.starts_on, r.ends_on, r.sprint_days)).fetch_one(&state.db).await?;
    let shift = match (s0, b.starts_on) {
        (Some(a), Some(b)) => chrono::Duration::days((b - a).num_days()),
        _ => chrono::Duration::zero(),
    };
    let (s1, e1) = (b.starts_on.or(s0), e0.map(|e| e + shift));
    let mut tx = state.db.begin().await?;
    let new: Uuid = sqlx::query_scalar(
        "INSERT INTO projects (name, description, visibility, color, created_by, kind, starts_on, ends_on, sprint_days, scrum_master_id)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, CASE WHEN $6 = 'scrum' THEN $5 END) RETURNING id",
    )
    .bind(&name)
    .bind(desc)
    .bind(vis)
    .bind(color)
    .bind(auth.id)
    .bind(&kind)
    .bind(s1)
    .bind(e1)
    .bind(sprint_days)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query!("INSERT INTO project_members (project_id, user_id, role) VALUES ($1, $2, 'owner')", new, auth.id).execute(&mut *tx).await?;
    if b.members {
        sqlx::query!("INSERT INTO project_members (project_id, user_id, role) SELECT $2, pm.user_id, pm.role FROM project_members pm JOIN users u ON u.id = pm.user_id
             WHERE pm.project_id = $1 AND u.is_active ON CONFLICT DO NOTHING", id, new)
        .execute(&mut *tx)
        .await?;
    }
    // Columns, keeping a map from old to new.
    let stages: Vec<(Uuid, String, String, f64, Option<String>)> =
        sqlx::query!(r#"SELECT id, name, color, sort, status FROM project_stages WHERE project_id = $1"#, id).map(|r| (r.id, r.name, r.color, r.sort, r.status)).fetch_all(&mut *tx).await?;
    let mut stage_map = std::collections::HashMap::new();
    for (sid, n, c, sort, status) in stages {
        let nid: Uuid = sqlx::query_scalar!("INSERT INTO project_stages (project_id, name, color, sort, status) VALUES ($1, $2, $3, $4, $5) RETURNING id", new, n, c, sort, status)
            .fetch_one(&mut *tx)
            .await?;
        stage_map.insert(sid, nid);
    }
    for (old, nid) in &stage_map {
        sqlx::query!("INSERT INTO project_stage_rules (project_id, stage_id, kind, params, sort, created_by) SELECT $1, $2, kind, params, sort, $4 FROM project_stage_rules WHERE stage_id = $3", new, nid, old, auth.id)
            .execute(&mut *tx)
            .await?;
        sqlx::query!("INSERT INTO project_stage_triggers (project_id, stage_id, event) SELECT $1, $2, event FROM project_stage_triggers WHERE stage_id = $3", new, nid, old)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query!("INSERT INTO task_field_defs (project_id, name, kind, options, required, show_in_list, sort) SELECT $2, name, kind, options, required, show_in_list, sort FROM task_field_defs WHERE project_id = $1", id, new)
        .execute(&mut *tx)
        .await?;
    sqlx::query!("INSERT INTO scrum_dod_items (project_id, title, sort) SELECT $2, title, sort FROM scrum_dod_items WHERE project_id = $1", id, new).execute(&mut *tx).await?;
    let mut copied = 0;
    if b.tasks {
        // Parents before children, so subtasks find their new parent.
        #[allow(clippy::type_complexity)]
        let tasks: Vec<(Uuid, Option<Uuid>, String, Option<String>, String, Uuid, Option<chrono::DateTime<Utc>>, Option<chrono::DateTime<Utc>>, bool, Option<i32>, Option<Uuid>, f64, Option<f64>, bool, bool)> = sqlx::query!(r#"WITH RECURSIVE tree AS (
                SELECT t.*, 0 AS depth FROM tasks t WHERE t.project_id = $1 AND (t.parent_id IS NULL OR t.parent_id NOT IN (SELECT id FROM tasks WHERE project_id = $1))
                UNION ALL SELECT c.*, tree.depth + 1 FROM tasks c JOIN tree ON c.parent_id = tree.id WHERE c.project_id = $1)
             SELECT id AS "id!", parent_id AS "parent_id?", title AS "title!", description AS "description?", priority AS "priority!", responsible_id AS "responsible_id!", start_date AS "start_date?", deadline AS "deadline?", requires_review AS "requires_review!", time_estimate_secs AS "time_estimate_secs?", stage_id AS "stage_id?", position AS "position!",
                    story_points AS "story_points?", requires_result AS "requires_result!", skip_nonworking AS "skip_nonworking!"
             FROM tree ORDER BY depth, position, created_at LIMIT 2000"#, id).map(|r| (r.id, r.parent_id, r.title, r.description, r.priority, r.responsible_id, r.start_date, r.deadline, r.requires_review, r.time_estimate_secs, r.stage_id, r.position, r.story_points, r.requires_result, r.skip_nonworking))
        .fetch_all(&mut *tx)
        .await?;
        let mut task_map = std::collections::HashMap::new();
        for (tid, parent, title, d, prio, resp, start, deadline, review, est, _stage, pos, points, needs_result, skip) in tasks {
            let number: i64 = sqlx::query_scalar!("UPDATE counters SET value = value + 1 WHERE name = 'task_number' RETURNING value").fetch_one(&mut *tx).await?;
            let resp_active: bool = sqlx::query_scalar!("SELECT is_active FROM users WHERE id = $1", resp).fetch_one(&mut *tx).await?;
            let nid: Uuid = sqlx::query_scalar(
                "INSERT INTO tasks (title, description, priority, responsible_id, created_by, project_id, parent_id, number, start_date, deadline,
                                    requires_review, time_estimate_secs, stage_id, position, story_points, requires_result, skip_nonworking)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12,
                         COALESCE($13, (SELECT id FROM project_stages WHERE project_id = $6 ORDER BY (status IS NOT DISTINCT FROM 'new') DESC, sort LIMIT 1)), $14, $15, $16, $17)
                 RETURNING id",
            )
            .bind(&title)
            .bind(d)
            .bind(prio)
            .bind(if resp_active { resp } else { auth.id })
            .bind(auth.id)
            .bind(new)
            .bind(parent.and_then(|p| task_map.get(&p).copied()))
            .bind(number)
            .bind(start.map(|s| s + shift))
            .bind(deadline.map(|s| s + shift))
            .bind(review)
            .bind(est)
            // Copies start over in the "to do" column.
            .bind(None::<Uuid>)
            .bind(pos)
            .bind(points)
            .bind(needs_result)
            .bind(skip)
            .fetch_one(&mut *tx)
            .await?;
            sqlx::query!("INSERT INTO task_checklist_items (task_id, title, sort, assignee_id) SELECT $2, title, sort, assignee_id FROM task_checklist_items WHERE task_id = $1", tid, nid)
                .execute(&mut *tx)
                .await?;
            sqlx::query!("INSERT INTO task_members (task_id, user_id, role) SELECT $2, m.user_id, m.role FROM task_members m JOIN users u ON u.id = m.user_id WHERE m.task_id = $1 AND u.is_active", tid, nid)
                .execute(&mut *tx)
                .await?;
            sqlx::query!("INSERT INTO task_field_values (task_id, field_id, value)
                 SELECT $2, nd.id, v.value FROM task_field_values v JOIN task_field_defs od ON od.id = v.field_id
                 JOIN task_field_defs nd ON nd.name = od.name AND nd.kind = od.kind AND nd.project_id IS NOT DISTINCT FROM (CASE WHEN od.project_id IS NULL THEN NULL ELSE $3::uuid END)
                 WHERE v.task_id = $1", tid, nid, new)
            .execute(&mut *tx)
            .await?;
            sqlx::query!("INSERT INTO task_history (task_id, actor_id, field, new_value) VALUES ($1, $2, 'created', $3)", nid, auth.id, &title).execute(&mut *tx).await?;
            task_map.insert(tid, nid);
            copied += 1;
        }
    }
    audit::record(&mut *tx, Some(auth.id), "project.copy", "project", Some(new.to_string()), json!({ "from": id, "name": name, "tasks": copied }), ip.as_deref()).await;
    tx.commit().await?;
    Ok(Json(load_detail(&state, &auth, new).await?))
}
