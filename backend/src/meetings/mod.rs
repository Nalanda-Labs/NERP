//! Meetings: plan them with an agenda, hold them recording decisions and attendance, close
//! them with minutes, and plan the next one in the series.
//!
//! - **People:** the owner runs the meeting, the keeper writes the minutes, members take
//!   part. A meeting of a workgroup is also visible to the workgroup's members.
//! - **States:** *prepare* (the agenda is written; members propose topics and the people
//!   responsible for a topic write their reports), *in progress* (decisions are recorded per
//!   topic, attendance is marked, tasks are made from topics), *closed* (the minutes are
//!   final and sent to everyone).
//! - **Series:** "plan the next meeting" copies the people and carries every unfinished topic
//!   to the new agenda. A topic keeps its history: what was reported and decided each time.
//! - The meeting is an event on the participants' calendars, kept in step with its time,
//!   place and people.
//!
//! Everyone can organise meetings.

mod agenda;

use axum::{
    extract::{Path, Query, State},
    routing::{get, post, put},
    Json, Router,
};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::FromRow;
use uuid::Uuid;

use crate::{
    auth::{permissions as perm, AuthUser},
    error::{AppError, AppResult},
    notify::{notify, NewNotification},
    routes::{clean, required},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/meetings", get(list).post(create))
        .route("/meetings/{id}", get(get_meeting).patch(update).delete(remove))
        .route("/meetings/{id}/members", put(set_members))
        .route("/meetings/{id}/attendance", post(attendance))
        .route("/meetings/{id}/start", post(start))
        .route("/meetings/{id}/close", post(close))
        .route("/meetings/{id}/reopen", post(reopen))
        .route("/meetings/{id}/next", post(plan_next))
        .merge(agenda::router())
}

#[derive(FromRow, Serialize, Clone)]
pub(crate) struct Meeting {
    pub id: Uuid,
    pub title: String,
    pub description: Option<String>,
    pub place: Option<String>,
    pub starts_at: DateTime<Utc>,
    pub duration_minutes: i32,
    pub timezone: String,
    pub state: String,
    pub project_id: Option<Uuid>,
    pub parent_id: Option<Uuid>,
    pub minutes: Option<String>,
    pub event_id: Option<Uuid>,
    pub file_ids: Vec<Uuid>,
    pub started_at: Option<DateTime<Utc>>,
    pub closed_at: Option<DateTime<Utc>>,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

const COLS: &str = "id, title, description, place, starts_at, duration_minutes, timezone, state, project_id, parent_id, minutes, event_id, file_ids,
    started_at, closed_at, created_by, created_at";

pub(crate) async fn load(state: &AppState, id: Uuid) -> AppResult<Meeting> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {COLS} FROM meetings WHERE id = $1")))
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound("meeting"))
}

/// The person's part in a meeting: their role, or "viewer" (a member of its workgroup, or an
/// administrator), or nothing.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Part {
    Owner,
    Keeper,
    Member,
    Viewer,
}

impl Part {
    /// Runs the meeting: changes it, records decisions, starts and closes it.
    pub fn runs(self) -> bool {
        matches!(self, Part::Owner | Part::Keeper)
    }
    pub fn takes_part(self) -> bool {
        self != Part::Viewer
    }
}

pub(crate) async fn part(state: &AppState, auth: &AuthUser, m: &Meeting) -> AppResult<Part> {
    let role: Option<String> = sqlx::query_scalar!("SELECT role FROM meeting_members WHERE meeting_id = $1 AND user_id = $2", m.id, auth.id)
        .fetch_optional(&state.db)
        .await?;
    match role.as_deref() {
        Some("owner") => return Ok(Part::Owner),
        Some("keeper") => return Ok(Part::Keeper),
        Some(_) => return Ok(Part::Member),
        None => {}
    }
    if auth.can(perm::ALL) {
        return Ok(Part::Viewer);
    }
    if let Some(p) = m.project_id {
        let member: bool = sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM project_members WHERE project_id = $1 AND user_id = $2) AS "v!""#, p, auth.id)
            .fetch_one(&state.db)
            .await?;
        if member {
            return Ok(Part::Viewer);
        }
    }
    Err(AppError::NotFound("meeting"))
}

/// Loads a meeting the person may see, with their part in it.
pub(crate) async fn visible(state: &AppState, auth: &AuthUser, id: Uuid) -> AppResult<(Meeting, Part)> {
    let m = load(state, id).await?;
    let p = part(state, auth, &m).await?;
    Ok((m, p))
}

fn not_closed(m: &Meeting) -> AppResult<()> {
    if m.state == "closed" { Err(AppError::bad("the meeting is closed; reopen it to change it")) } else { Ok(()) }
}

async fn members_of(state: &AppState, id: Uuid) -> AppResult<Vec<(Uuid, String)>> {
    Ok(sqlx::query!(r#"SELECT user_id, role FROM meeting_members WHERE meeting_id = $1"#, id).map(|r| (r.user_id, r.role)).fetch_all(&state.db).await?)
}

fn link(id: Uuid) -> String {
    format!("/meetings/{id}")
}

fn when(m: &Meeting) -> String {
    let tz: chrono_tz::Tz = m.timezone.parse().unwrap_or(chrono_tz::UTC);
    m.starts_at.with_timezone(&tz).format("%a %-d %b, %H:%M").to_string()
}

// ---------------------------------------------------------------------------
// The calendar event, kept in step with the meeting
// ---------------------------------------------------------------------------

/// Creates or updates the meeting's calendar event, as its owner. Guests: everyone but the owner.
async fn sync_event(state: &AppState, id: Uuid) -> AppResult<()> {
    let m = load(state, id).await?;
    let members = members_of(state, id).await?;
    let Some(owner) = members.iter().find(|(_, r)| r == "owner").map(|(u, _)| *u) else { return Ok(()) };
    let Some(owner_auth) = crate::auth::acting_as(state, owner).await? else { return Ok(()) };
    let input = json!({
        "title": m.title,
        "description": format!("{}{}/meetings/{}", m.description.as_deref().map(|d| format!("{d}\n\n")).unwrap_or_default(), state.config.public_url, m.id),
        "location": m.place,
        "start": m.starts_at.to_rfc3339(),
        "end": (m.starts_at + Duration::minutes(m.duration_minutes as i64)).to_rfc3339(),
        "timezone": m.timezone,
        "attendees": members.iter().filter(|(u, _)| *u != owner).map(|(u, _)| *u).collect::<Vec<_>>(),
    });
    match m.event_id {
        Some(ev) => {
            // The event may belong to an earlier owner: then the new owner gets a fresh one.
            if crate::calendar::update_event(state, &owner_auth, ev, input.clone()).await.is_ok() {
                return Ok(());
            }
            let ev = crate::calendar::create_event(state, &owner_auth, input).await?;
            set_event(state, id, &ev).await
        }
        None => {
            let ev = crate::calendar::create_event(state, &owner_auth, input).await?;
            set_event(state, id, &ev).await
        }
    }
}

async fn set_event(state: &AppState, id: Uuid, ev: &Value) -> AppResult<()> {
    let ev_id: Option<Uuid> = ev["id"].as_str().and_then(|s| s.parse().ok()).or_else(|| ev["event"]["id"].as_str().and_then(|s| s.parse().ok()));
    sqlx::query!("UPDATE meetings SET event_id = $2 WHERE id = $1", id, ev_id).execute(&state.db).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Listing and creating
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct ListQuery {
    /// upcoming | past | all
    scope: Option<String>,
    project_id: Option<Uuid>,
    q: Option<String>,
}

async fn list(State(state): State<AppState>, auth: AuthUser, Query(q): Query<ListQuery>) -> AppResult<Json<Value>> {
    let scope = q.scope.as_deref().unwrap_or("upcoming");
    let when = match scope {
        "upcoming" => "m.state <> 'closed' AND m.starts_at + make_interval(mins => m.duration_minutes) >= now() - interval '1 day'",
        "past" => "(m.state = 'closed' OR m.starts_at + make_interval(mins => m.duration_minutes) < now() - interval '1 day')",
        "all" => "TRUE",
        _ => return Err(AppError::bad("scope is upcoming, past or all")),
    };
    let order = if scope == "upcoming" { "m.starts_at ASC" } else { "m.starts_at DESC" };
    let rows: Vec<(Uuid, String, DateTime<Utc>, i32, String, Option<String>, Option<Uuid>, Option<String>, Option<String>, i64, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT m.id, m.title, m.starts_at, m.duration_minutes, m.state, m.place, m.project_id, p.name, me.role,
                (SELECT count(*) FROM meeting_members x WHERE x.meeting_id = m.id),
                (SELECT count(*) FROM meeting_agenda a WHERE a.meeting_id = m.id AND a.parent_id IS NULL)
         FROM meetings m
         LEFT JOIN projects p ON p.id = m.project_id
         LEFT JOIN meeting_members me ON me.meeting_id = m.id AND me.user_id = $1
         WHERE {when}
           AND (me.user_id IS NOT NULL OR $2 OR EXISTS (SELECT 1 FROM project_members pm WHERE pm.project_id = m.project_id AND pm.user_id = $1))
           AND ($3::uuid IS NULL OR m.project_id = $3)
           AND ($4::text IS NULL OR m.title ILIKE '%' || $4 || '%')
         ORDER BY {order} LIMIT 200"
    )))
    .bind(auth.id)
    .bind(auth.can(perm::ALL) && q.project_id.is_some())
    .bind(q.project_id)
    .bind(clean(q.q))
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({ "items": rows.into_iter().map(|(id, title, starts_at, dur, st, place, pid, pname, role, people, topics)| json!({
        "id": id, "title": title, "starts_at": starts_at, "duration_minutes": dur, "state": st, "place": place,
        "project": pid.map(|p| json!({ "id": p, "name": pname })), "my_role": role, "people": people, "topics": topics,
    })).collect::<Vec<_>>() })))
}

#[derive(Deserialize)]
struct MemberInput {
    user_id: Uuid,
    role: Option<String>,
}

#[derive(Deserialize)]
struct MeetingInput {
    title: Option<String>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    description: Option<Option<String>>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    place: Option<Option<String>>,
    starts_at: Option<DateTime<Utc>>,
    duration_minutes: Option<i32>,
    timezone: Option<String>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    project_id: Option<Option<Uuid>>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    minutes: Option<Option<String>>,
    file_ids: Option<Vec<Uuid>>,
    /// When creating: who takes part (the creator is the owner).
    members: Option<Vec<MemberInput>>,
}

fn check_duration(d: i32) -> AppResult<i32> {
    if (5..=1440).contains(&d) { Ok(d) } else { Err(AppError::bad("a meeting lasts 5 minutes to 24 hours")) }
}

fn check_tz(tz: &str) -> AppResult<String> {
    tz.parse::<chrono_tz::Tz>().map(|_| tz.to_string()).map_err(|_| AppError::bad("unknown time zone"))
}

async fn check_project(state: &AppState, auth: &AuthUser, project: Option<Uuid>) -> AppResult<()> {
    if let Some(p) = project {
        let ok: bool = sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM project_members WHERE project_id = $1 AND user_id = $2 AND role <> 'viewer') AS "v!""#, p, auth.id)
            .fetch_one(&state.db)
            .await?;
        if !ok && !auth.can(perm::ALL) {
            return Err(AppError::bad("you can only plan meetings for workgroups you work in"));
        }
    }
    Ok(())
}

async fn own_files(state: &AppState, auth: &AuthUser, ids: &[Uuid], current: &[Uuid]) -> AppResult<()> {
    if ids.len() > 30 {
        return Err(AppError::bad("at most 30 files"));
    }
    for f in ids.iter().filter(|f| !current.contains(f)) {
        let owner: Option<Option<Uuid>> = sqlx::query_scalar!(r#"SELECT owner_id AS "owner_id?" FROM files WHERE id = $1"#, f).fetch_optional(&state.db).await?;
        if owner.flatten() != Some(auth.id) {
            return Err(AppError::NotFound("file"));
        }
    }
    Ok(())
}

/// Checks a member list: known active people, valid roles, the owner given separately.
async fn check_members(state: &AppState, list: &[MemberInput], owner: Uuid) -> AppResult<Vec<(Uuid, String)>> {
    if list.len() > 200 {
        return Err(AppError::bad("at most 200 people"));
    }
    let mut out = vec![(owner, "owner".to_string())];
    for m in list {
        if m.user_id == owner || out.iter().any(|(u, _)| *u == m.user_id) {
            continue;
        }
        let role = m.role.clone().unwrap_or_else(|| "member".into());
        if !["keeper", "member"].contains(&role.as_str()) {
            return Err(AppError::bad("a person is the keeper (writes the minutes) or a member"));
        }
        let ok: bool = sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM users WHERE id = $1 AND is_active AND NOT is_system) AS "v!""#, m.user_id).fetch_one(&state.db).await?;
        if !ok {
            return Err(AppError::bad("unknown person"));
        }
        out.push((m.user_id, role));
    }
    Ok(out)
}

async fn create(State(state): State<AppState>, auth: AuthUser, Json(b): Json<MeetingInput>) -> AppResult<Json<Value>> {
    let title = required("title", b.title.as_deref().unwrap_or(""), 300)?;
    let starts_at = b.starts_at.ok_or_else(|| AppError::bad("choose when the meeting starts"))?;
    let duration = check_duration(b.duration_minutes.unwrap_or(60))?;
    let tz = check_tz(b.timezone.as_deref().unwrap_or("UTC"))?;
    let project = b.project_id.flatten();
    check_project(&state, &auth, project).await?;
    let files = b.file_ids.unwrap_or_default();
    own_files(&state, &auth, &files, &[]).await?;
    let members = check_members(&state, &b.members.unwrap_or_default(), auth.id).await?;
    let id = new_meeting(&state, &auth, NewMeeting {
        title,
        description: clean(b.description.flatten()),
        place: clean(b.place.flatten()),
        starts_at,
        duration,
        timezone: tz,
        project,
        parent: None,
        files,
        members,
    })
    .await?;
    get_meeting(State(state), auth, Path(id)).await
}

struct NewMeeting {
    title: String,
    description: Option<String>,
    place: Option<String>,
    starts_at: DateTime<Utc>,
    duration: i32,
    timezone: String,
    project: Option<Uuid>,
    parent: Option<Uuid>,
    files: Vec<Uuid>,
    members: Vec<(Uuid, String)>,
}

async fn new_meeting(state: &AppState, auth: &AuthUser, n: NewMeeting) -> AppResult<Uuid> {
    let mut tx = state.db.begin().await?;
    let id: Uuid = sqlx::query_scalar!("INSERT INTO meetings (title, description, place, starts_at, duration_minutes, timezone, project_id, parent_id, file_ids, created_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) RETURNING id", &n.title, n.description.as_deref(), n.place.as_deref(), n.starts_at, n.duration, &n.timezone, n.project, n.parent, &n.files, auth.id)
    .fetch_one(&mut *tx)
    .await?;
    for (u, role) in &n.members {
        sqlx::query!("INSERT INTO meeting_members (meeting_id, user_id, role) VALUES ($1, $2, $3)", id, u, role).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    sync_event(state, id).await?;
    let m = load(state, id).await?;
    let others: Vec<Uuid> = n.members.iter().map(|(u, _)| *u).filter(|u| *u != auth.id).collect();
    notify(state, &others, NewNotification {
        kind: "meeting.invited",
        title: format!("Meeting “{}”, {}", m.title, when(&m)),
        body: Some("You are invited. Add topics to the agenda before it starts.".into()),
        link: Some(link(id)),
        actor: Some(auth.id),
    })
    .await;
    Ok(id)
}

// ---------------------------------------------------------------------------
// One meeting
// ---------------------------------------------------------------------------

async fn names(state: &AppState, ids: &[Uuid]) -> AppResult<Vec<Value>> {
    let rows: Vec<(Uuid, String, Option<Uuid>)> = sqlx::query!(r#"SELECT id AS "id!", first_name || ' ' || last_name AS "c1!", avatar_file_id AS "avatar_file_id?" FROM users WHERE id = ANY($1)"#, ids).map(|r| (r.id, r.c1, r.avatar_file_id))
        .fetch_all(&state.db)
        .await?;
    Ok(ids.iter().filter_map(|i| rows.iter().find(|r| r.0 == *i)).map(|(id, name, av)| json!({ "id": id, "name": name, "avatar_file_id": av })).collect())
}

pub(crate) async fn meeting_json(state: &AppState, auth: &AuthUser, m: &Meeting, p: Part) -> AppResult<Value> {
    let members: Vec<(Uuid, String, Option<bool>, String, Option<Uuid>)> = sqlx::query!(r#"SELECT mm.user_id AS "user_id!", mm.role AS "role!", mm.attended AS "attended?", u.first_name || ' ' || u.last_name AS "c3!", u.avatar_file_id AS "avatar_file_id?"
         FROM meeting_members mm JOIN users u ON u.id = mm.user_id WHERE mm.meeting_id = $1
         ORDER BY CASE mm.role WHEN 'owner' THEN 0 WHEN 'keeper' THEN 1 ELSE 2 END, u.first_name, u.last_name"#, m.id).map(|r| (r.user_id, r.role, r.attended, r.c3, r.avatar_file_id))
    .fetch_all(&state.db)
    .await?;
    let files: Vec<(Uuid, String, i64)> = sqlx::query!(r#"SELECT id, original_name, size_bytes FROM files WHERE id = ANY($1)"#, &m.file_ids).map(|r| (r.id, r.original_name, r.size_bytes)).fetch_all(&state.db).await?;
    let project: Option<(Uuid, String)> = match m.project_id {
        Some(p) => sqlx::query_as("SELECT id, name FROM projects WHERE id = $1").bind(p).fetch_optional(&state.db).await?,
        None => None,
    };
    let previous: Option<(Uuid, String, DateTime<Utc>)> = match m.parent_id {
        Some(pid) => sqlx::query_as("SELECT id, title, starts_at FROM meetings WHERE id = $1").bind(pid).fetch_optional(&state.db).await?,
        None => None,
    };
    let next: Option<(Uuid, String, DateTime<Utc>)> =
        sqlx::query!(r#"SELECT id, title, starts_at FROM meetings WHERE parent_id = $1 ORDER BY starts_at LIMIT 1"#, m.id).map(|r| (r.id, r.title, r.starts_at)).fetch_optional(&state.db).await?;
    let mut v = serde_json::to_value(m).unwrap();
    v["ends_at"] = json!(m.starts_at + Duration::minutes(m.duration_minutes as i64));
    v["members"] = json!(members.into_iter().map(|(id, role, att, name, av)| json!({ "id": id, "role": role, "attended": att, "name": name, "avatar_file_id": av })).collect::<Vec<_>>());
    v["files"] = json!(files.into_iter().map(|(id, name, size)| json!({ "id": id, "name": name, "size": size })).collect::<Vec<_>>());
    v["project"] = json!(project.map(|(id, name)| json!({ "id": id, "name": name })));
    v["previous"] = json!(previous.map(|(id, t, s)| json!({ "id": id, "title": t, "starts_at": s })));
    v["next"] = json!(next.map(|(id, t, s)| json!({ "id": id, "title": t, "starts_at": s })));
    v["agenda"] = agenda::agenda_json(state, m.id).await?;
    v["my_role"] = json!(match p {
        Part::Owner => "owner",
        Part::Keeper => "keeper",
        Part::Member => "member",
        Part::Viewer => "viewer",
    });
    v["can"] = json!({ "run": p.runs(), "propose": p.takes_part() && m.state != "closed", "manage_people": p == Part::Owner, "delete": p == Part::Owner || auth.can(perm::ALL) });
    Ok(v)
}

async fn get_meeting(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let (m, p) = visible(&state, &auth, id).await?;
    Ok(Json(meeting_json(&state, &auth, &m, p).await?))
}

async fn update(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<MeetingInput>) -> AppResult<Json<Value>> {
    let (m, p) = visible(&state, &auth, id).await?;
    if !p.runs() {
        return Err(AppError::Forbidden);
    }
    // Closed meetings keep their record; only the minutes text may still be corrected.
    let only_minutes = b.title.is_none() && b.description.is_none() && b.place.is_none() && b.starts_at.is_none() && b.duration_minutes.is_none()
        && b.timezone.is_none() && b.project_id.is_none() && b.file_ids.is_none();
    if m.state == "closed" && !only_minutes {
        return Err(AppError::bad("the meeting is closed; reopen it to change it"));
    }
    let title = b.title.as_deref().map(|t| required("title", t, 300)).transpose()?;
    let duration = b.duration_minutes.map(check_duration).transpose()?;
    let tz = b.timezone.as_deref().map(check_tz).transpose()?;
    if let Some(p) = b.project_id {
        check_project(&state, &auth, p).await?;
    }
    if let Some(f) = &b.file_ids {
        own_files(&state, &auth, f, &m.file_ids).await?;
    }
    let minutes = b.minutes.map(clean);
    if minutes.as_ref().and_then(|x| x.as_ref()).is_some_and(|x| x.chars().count() > 200_000) {
        return Err(AppError::bad("the minutes are too long"));
    }
    sqlx::query!("UPDATE meetings SET title = COALESCE($2, title), description = CASE WHEN $3 THEN $4 ELSE description END,
             place = CASE WHEN $5 THEN $6 ELSE place END, starts_at = COALESCE($7, starts_at), duration_minutes = COALESCE($8, duration_minutes),
             timezone = COALESCE($9, timezone), project_id = CASE WHEN $10 THEN $11 ELSE project_id END,
             minutes = CASE WHEN $12 THEN $13 ELSE minutes END, file_ids = COALESCE($14, file_ids), updated_at = now()
         WHERE id = $1", id, title, b.description.is_some(), clean(b.description.flatten()), b.place.is_some(), clean(b.place.flatten()), b.starts_at, duration, tz, b.project_id.is_some(), b.project_id.flatten(), minutes.is_some(), minutes.flatten(), b.file_ids.as_deref())
    .execute(&state.db)
    .await?;
    let moved = b.starts_at.is_some_and(|s| s != m.starts_at);
    if !only_minutes {
        sync_event(&state, id).await?;
    }
    if moved && m.state == "prepare" {
        let m2 = load(&state, id).await?;
        let others: Vec<Uuid> = members_of(&state, id).await?.into_iter().map(|(u, _)| u).filter(|u| *u != auth.id).collect();
        notify(&state, &others, NewNotification {
            kind: "meeting.moved",
            title: format!("Meeting “{}” moved to {}", m2.title, when(&m2)),
            body: None,
            link: Some(link(id)),
            actor: Some(auth.id),
        })
        .await;
    }
    get_meeting(State(state), auth, Path(id)).await
}

async fn set_members(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(list): Json<Vec<MemberInput>>) -> AppResult<Json<Value>> {
    let (m, p) = visible(&state, &auth, id).await?;
    if p != Part::Owner {
        return Err(AppError::Forbidden);
    }
    not_closed(&m)?;
    // The owner may hand the meeting over: exactly one owner in the new list, else the current one.
    let owners: Vec<&MemberInput> = list.iter().filter(|x| x.role.as_deref() == Some("owner")).collect();
    if owners.len() > 1 {
        return Err(AppError::bad("a meeting has one owner"));
    }
    let owner = owners.first().map(|o| o.user_id).unwrap_or(auth.id);
    let rest: Vec<MemberInput> = list.into_iter().filter(|x| x.role.as_deref() != Some("owner")).collect();
    let members = check_members(&state, &rest, owner).await?;
    let before: Vec<Uuid> = members_of(&state, id).await?.into_iter().map(|(u, _)| u).collect();
    let mut tx = state.db.begin().await?;
    sqlx::query!("DELETE FROM meeting_members WHERE meeting_id = $1 AND NOT (user_id = ANY($2))", id, &(members.iter().map(|(u, _)| *u).collect::<Vec<_>>()))
        .execute(&mut *tx)
        .await?;
    for (u, role) in &members {
        sqlx::query!("INSERT INTO meeting_members (meeting_id, user_id, role) VALUES ($1, $2, $3) ON CONFLICT (meeting_id, user_id) DO UPDATE SET role = EXCLUDED.role", id, u, role)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    sync_event(&state, id).await?;
    let added: Vec<Uuid> = members.iter().map(|(u, _)| *u).filter(|u| !before.contains(u) && *u != auth.id).collect();
    notify(&state, &added, NewNotification {
        kind: "meeting.invited",
        title: format!("Meeting “{}”, {}", m.title, when(&m)),
        body: Some("You are invited. Add topics to the agenda before it starts.".into()),
        link: Some(link(id)),
        actor: Some(auth.id),
    })
    .await;
    get_meeting(State(state), auth, Path(id)).await
}

#[derive(Deserialize)]
struct AttendanceInput {
    user_id: Uuid,
    attended: Option<bool>,
}

async fn attendance(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<AttendanceInput>) -> AppResult<Json<Value>> {
    let (m, p) = visible(&state, &auth, id).await?;
    if !p.runs() {
        return Err(AppError::Forbidden);
    }
    not_closed(&m)?;
    let n = sqlx::query!("UPDATE meeting_members SET attended = $3 WHERE meeting_id = $1 AND user_id = $2", id, b.user_id, b.attended).execute(&state.db).await?.rows_affected();
    if n == 0 {
        return Err(AppError::bad("that person is not in this meeting"));
    }
    get_meeting(State(state), auth, Path(id)).await
}

async fn start(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let (m, p) = visible(&state, &auth, id).await?;
    if !p.runs() {
        return Err(AppError::Forbidden);
    }
    if m.state != "prepare" {
        return Err(AppError::bad("only a meeting being prepared can start"));
    }
    sqlx::query!("UPDATE meetings SET state = 'in_progress', started_at = now(), updated_at = now() WHERE id = $1", id).execute(&state.db).await?;
    get_meeting(State(state), auth, Path(id)).await
}

async fn close(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let (m, p) = visible(&state, &auth, id).await?;
    if !p.runs() {
        return Err(AppError::Forbidden);
    }
    if m.state != "in_progress" {
        return Err(AppError::bad("start the meeting before closing it"));
    }
    sqlx::query!("UPDATE meetings SET state = 'closed', closed_at = now(), updated_at = now() WHERE id = $1", id).execute(&state.db).await?;
    let everyone: Vec<Uuid> = members_of(&state, id).await?.into_iter().map(|(u, _)| u).collect();
    let decided: i64 = sqlx::query_scalar!(r#"SELECT count(*) AS "v!" FROM meeting_agenda WHERE meeting_id = $1 AND decision IS NOT NULL"#, id).fetch_one(&state.db).await?;
    notify(&state, &everyone, NewNotification {
        kind: "meeting.minutes",
        title: format!("Minutes of “{}” are ready", m.title),
        body: Some(format!("{decided} decision{} recorded.", if decided == 1 { "" } else { "s" })),
        link: Some(format!("/meetings/{id}/minutes")),
        actor: Some(auth.id),
    })
    .await;
    get_meeting(State(state), auth, Path(id)).await
}

async fn reopen(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let (m, p) = visible(&state, &auth, id).await?;
    if p != Part::Owner {
        return Err(AppError::Forbidden);
    }
    if m.state != "closed" {
        return Err(AppError::bad("the meeting is not closed"));
    }
    sqlx::query!("UPDATE meetings SET state = 'in_progress', closed_at = NULL, updated_at = now() WHERE id = $1", id).execute(&state.db).await?;
    get_meeting(State(state), auth, Path(id)).await
}

#[derive(Deserialize)]
struct NextInput {
    starts_at: DateTime<Utc>,
    duration_minutes: Option<i32>,
    title: Option<String>,
}

/// The next meeting in the series: same people, place and workgroup; every unfinished topic
/// (and its unfinished sub-points) goes on the new agenda.
async fn plan_next(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<NextInput>) -> AppResult<Json<Value>> {
    let (m, p) = visible(&state, &auth, id).await?;
    if !p.runs() {
        return Err(AppError::Forbidden);
    }
    let exists: bool = sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM meetings WHERE parent_id = $1) AS "v!""#, id).fetch_one(&state.db).await?;
    if exists {
        return Err(AppError::Conflict("the next meeting is already planned".into()));
    }
    if b.starts_at <= m.starts_at {
        return Err(AppError::bad("the next meeting comes after this one"));
    }
    let members = members_of(&state, id).await?;
    let owner = members.iter().find(|(_, r)| r == "owner").map(|(u, _)| *u).unwrap_or(auth.id);
    let new_id = new_meeting(&state, &auth, NewMeeting {
        title: b.title.as_deref().map(|t| required("title", t, 300)).transpose()?.unwrap_or_else(|| m.title.clone()),
        description: m.description.clone(),
        place: m.place.clone(),
        starts_at: b.starts_at,
        duration: check_duration(b.duration_minutes.unwrap_or(m.duration_minutes))?,
        timezone: m.timezone.clone(),
        project: m.project_id,
        parent: Some(id),
        files: vec![],
        members: std::iter::once((owner, "owner".to_string())).chain(members.into_iter().filter(|(u, _)| *u != owner)).collect(),
    })
    .await?;
    agenda::carry_over(&state, id, new_id).await?;
    get_meeting(State(state), auth, Path(new_id)).await
}

async fn remove(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let (m, p) = visible(&state, &auth, id).await?;
    if p != Part::Owner && !auth.can(perm::ALL) {
        return Err(AppError::Forbidden);
    }
    if let Some(ev) = m.event_id {
        let owner: Option<Uuid> = sqlx::query_scalar!("SELECT owner_id FROM cal_events WHERE id = $1", ev).fetch_optional(&state.db).await?;
        if let Some(o) = owner
            && let Some(a) = crate::auth::acting_as(&state, o).await? {
                let _ = crate::calendar::delete_event(&state, &a, ev).await;
            }
    }
    if m.state == "prepare" {
        let others: Vec<Uuid> = members_of(&state, id).await?.into_iter().map(|(u, _)| u).filter(|u| *u != auth.id).collect();
        notify(&state, &others, NewNotification {
            kind: "meeting.cancelled",
            title: format!("Meeting “{}” on {} was cancelled", m.title, when(&m)),
            body: None,
            link: None,
            actor: Some(auth.id),
        })
        .await;
    }
    sqlx::query!("DELETE FROM meetings WHERE id = $1", id).execute(&state.db).await?;
    Ok(Json(json!({ "ok": true })))
}

/// Files attached to a meeting or to a report on its agenda: for those who may see it.
pub async fn can_view_file(state: &AppState, auth: &AuthUser, file: Uuid) -> AppResult<bool> {
    let meetings: Vec<Uuid> = sqlx::query_scalar!(r#"SELECT id AS "id!" FROM meetings WHERE $1 = ANY(file_ids)
         UNION SELECT a.meeting_id FROM meeting_reports r JOIN meeting_agenda a ON a.id = r.agenda_id WHERE $1 = ANY(r.file_ids)"#, file)
    .fetch_all(&state.db)
    .await?;
    for id in meetings {
        if visible(state, auth, id).await.is_ok() {
            return Ok(true);
        }
    }
    Ok(false)
}
