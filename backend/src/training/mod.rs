//! Training: courses of chapters and lessons, questions and tests, a gradebook, assignments and
//! certificates.
//!
//! - A **course** is a draft until published. Published courses are open to everyone or to
//!   chosen people and departments (a department includes its sub-departments), who enrol
//!   themselves; people who manage training can also **assign** a course to people or
//!   departments with a due date (they are notified, and reminded two days before).
//! - **Lessons** (Markdown, files, a video link) sit in chapters and may open some days after
//!   the person enrolled. People mark lessons done; lessons can carry self-check questions.
//! - **Tests** draw questions from the course (or one lesson): single and multiple choice,
//!   putting items in order, and written answers that a person grades. See `attempts`.
//! - A course is **completed** when every active test is passed (or, without tests, every
//!   lesson is done); a certificate is issued then.
//!
//! "Manage training" (training.manage) creates and edits courses, assigns them, grades written
//! answers and sees the gradebook.

mod attempts;
mod content;

use anyhow::Result;
use axum::{
    extract::{Path, Query, State},
    routing::{delete, get, post},
    Json, Router,
};
use chrono::{DateTime, Duration, NaiveDate, Utc};
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

pub const REMIND_JOB: &str = "training.remind";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/training/courses", get(list_courses).post(create_course))
        .route("/training/courses/{id}", get(get_course).patch(update_course).delete(delete_course))
        .route("/training/courses/{id}/enrol", post(enrol).delete(leave))
        .route("/training/courses/{id}/assign", post(assign))
        .route("/training/courses/{id}/enrolments/{user}", delete(unenrol))
        .route("/training/courses/{id}/chapters", post(create_chapter))
        .route("/training/chapters/{id}", axum::routing::patch(update_chapter).delete(delete_chapter))
        .route("/training/courses/{id}/lessons", post(create_lesson))
        .route("/training/lessons/{id}", get(get_lesson).patch(update_lesson).delete(delete_lesson))
        .route("/training/lessons/{id}/complete", post(complete_lesson).delete(uncomplete_lesson))
        .route("/training/certificates", get(list_certificates))
        .route("/training/certificates/{id}", get(get_certificate).delete(revoke_certificate))
        .merge(content::router())
        .merge(attempts::router())
}

pub(crate) fn manager(auth: &AuthUser) -> bool {
    auth.can(perm::TRAINING)
}

fn require_manager(auth: &AuthUser) -> AppResult<()> {
    auth.require(perm::TRAINING)
}

const MY_DEPTS: &str = "WITH RECURSIVE mine AS (
    SELECT department_id AS id FROM user_departments WHERE user_id = $1
    UNION SELECT d.parent_id FROM departments d JOIN mine m ON d.id = m.id WHERE d.parent_id IS NOT NULL)
    SELECT id FROM mine";

/// SQL: course c is visible to user $1 (manager: $2). Drafts only to managers; published ones
/// to their audience and to anyone enrolled.
fn visible_sql() -> String {
    format!(
        "($2 OR (c.is_published AND (c.to_all
            OR EXISTS (SELECT 1 FROM training_access a WHERE a.course_id = c.id AND (a.user_id = $1 OR a.department_id IN ({MY_DEPTS})))
            OR EXISTS (SELECT 1 FROM training_enrolments e WHERE e.course_id = c.id AND e.user_id = $1))))"
    )
}

/// SQL: user $1 may enrol themselves in course c.
fn open_to_sql() -> String {
    format!(
        "(c.is_published AND (c.to_all OR EXISTS (SELECT 1 FROM training_access a WHERE a.course_id = c.id AND (a.user_id = $1 OR a.department_id IN ({MY_DEPTS})))))"
    )
}

#[derive(Serialize, FromRow, Clone)]
pub(crate) struct Course {
    pub id: Uuid,
    pub title: String,
    pub description: String,
    pub cover_file_id: Option<Uuid>,
    pub is_published: bool,
    pub to_all: bool,
    pub sort: i32,
    pub created_at: DateTime<Utc>,
}

const COURSE_COLS: &str = "c.id, c.title, c.description, c.cover_file_id, c.is_published, c.to_all, c.sort, c.created_at";

/// The course, if the user may see it.
pub(crate) async fn visible_course(state: &AppState, auth: &AuthUser, id: Uuid) -> AppResult<Course> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {COURSE_COLS} FROM training_courses c WHERE c.id = $3 AND {}", visible_sql())))
        .bind(auth.id)
        .bind(manager(auth))
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound("course"))
}

#[derive(Serialize, FromRow, Clone)]
pub(crate) struct Enrolment {
    pub source: String,
    pub due_on: Option<NaiveDate>,
    pub enrolled_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

pub(crate) async fn enrolment(state: &AppState, user: Uuid, course: Uuid) -> AppResult<Option<Enrolment>> {
    Ok(sqlx::query_as("SELECT source, due_on, enrolled_at, completed_at FROM training_enrolments WHERE course_id = $1 AND user_id = $2")
        .bind(course)
        .bind(user)
        .fetch_optional(&state.db)
        .await?)
}

/// Must be enrolled (people who manage training may look at everything).
pub(crate) async fn require_enrolled(state: &AppState, auth: &AuthUser, course: Uuid) -> AppResult<Option<Enrolment>> {
    let e = enrolment(state, auth.id, course).await?;
    if e.is_none() && !manager(auth) {
        return Err(AppError::bad("enrol in the course first"));
    }
    Ok(e)
}

/// When a lesson opens for someone enrolled at `enrolled`.
fn opens_at(enrolled: Option<DateTime<Utc>>, days: i32) -> Option<DateTime<Utc>> {
    if days == 0 {
        return None;
    }
    enrolled.map(|e| e + Duration::days(days as i64))
}

/// A person's progress through a course.
pub(crate) async fn progress(state: &AppState, user: Uuid, course: Uuid) -> AppResult<Value> {
    let (lessons, done): (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*), COUNT(p.user_id) FROM training_lessons l LEFT JOIN training_lesson_progress p ON p.lesson_id = l.id AND p.user_id = $2 WHERE l.course_id = $1",
    )
    .bind(course)
    .bind(user)
    .fetch_one(&state.db)
    .await?;
    let (tests, passed): (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*), COUNT(*) FILTER (WHERE EXISTS (SELECT 1 FROM training_attempts a WHERE a.test_id = t.id AND a.user_id = $2 AND a.passed))
         FROM training_tests t WHERE t.course_id = $1 AND t.is_active",
    )
    .bind(course)
    .bind(user)
    .fetch_one(&state.db)
    .await?;
    let total = lessons + tests;
    let percent = if total == 0 { 0 } else { (done + passed) * 100 / total };
    Ok(json!({ "lessons": lessons, "lessons_done": done, "tests": tests, "tests_passed": passed, "percent": percent }))
}

/// Completes the course for the person when everything is done, and issues the certificate.
pub(crate) async fn check_completion(state: &AppState, user: Uuid, course: Uuid) -> AppResult<()> {
    let Some(e) = enrolment(state, user, course).await? else { return Ok(()) };
    if e.completed_at.is_some() {
        return Ok(());
    }
    let p = progress(state, user, course).await?;
    let n = |k: &str| p[k].as_i64().unwrap_or(0);
    let complete = if n("tests") > 0 { n("tests_passed") == n("tests") } else { n("lessons") > 0 && n("lessons_done") == n("lessons") };
    if !complete {
        return Ok(());
    }
    // The best score of each active test.
    let (score, max): (i64, i64) = sqlx::query_as(
        "SELECT COALESCE(SUM(b.score), 0), COALESCE(SUM(b.max_score), 0) FROM (
            SELECT DISTINCT ON (a.test_id) a.score, a.max_score FROM training_attempts a JOIN training_tests t ON t.id = a.test_id
            WHERE t.course_id = $1 AND t.is_active AND a.user_id = $2 AND a.status = 'done' ORDER BY a.test_id, a.percent DESC, a.score DESC) b",
    )
    .bind(course)
    .bind(user)
    .fetch_one(&state.db)
    .await?;
    let updated = sqlx::query("UPDATE training_enrolments SET completed_at = now() WHERE course_id = $1 AND user_id = $2 AND completed_at IS NULL")
        .bind(course)
        .bind(user)
        .execute(&state.db)
        .await?;
    if updated.rows_affected() == 0 {
        return Ok(());
    }
    let cert: Option<Uuid> = sqlx::query_scalar(
        "INSERT INTO training_certificates (course_id, user_id, score, max_score) VALUES ($1, $2, $3, $4) ON CONFLICT (course_id, user_id) DO NOTHING RETURNING id",
    )
    .bind(course)
    .bind(user)
    .bind(score as i32)
    .bind(max as i32)
    .fetch_optional(&state.db)
    .await?;
    let title: String = sqlx::query_scalar("SELECT title FROM training_courses WHERE id = $1").bind(course).fetch_one(&state.db).await?;
    notify(state, &[user], NewNotification {
        kind: "training.completed",
        title: format!("You completed “{title}”"),
        body: Some(if cert.is_some() { "Your certificate is ready.".into() } else { "Well done.".into() }),
        link: Some(match cert { Some(c) => format!("/training/certificates/{c}"), None => format!("/training/courses/{course}") }),
        actor: None,
    })
    .await;
    Ok(())
}

/// Files of training: a course's cover for whoever sees the course; a lesson's files for people
/// enrolled in its course (and people who manage training).
pub async fn can_view_file(state: &AppState, auth: &AuthUser, file: Uuid) -> AppResult<bool> {
    let cover: bool = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT EXISTS (SELECT 1 FROM training_courses c WHERE c.cover_file_id = $3 AND {})", visible_sql())))
        .bind(auth.id)
        .bind(manager(auth))
        .bind(file)
        .fetch_one(&state.db)
        .await?;
    if cover {
        return Ok(true);
    }
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM training_lessons l WHERE $2 = ANY(l.file_ids)
            AND ($3 OR EXISTS (SELECT 1 FROM training_enrolments e WHERE e.course_id = l.course_id AND e.user_id = $1)))",
    )
    .bind(auth.id)
    .bind(file)
    .bind(manager(auth))
    .fetch_one(&state.db)
    .await?)
}

// ---------------------------------------------------------------------------
// Courses
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct ListQuery {
    /// all (default) | mine (enrolled) | manage (everything, for managers)
    filter: Option<String>,
}

async fn list_courses(State(state): State<AppState>, auth: AuthUser, Query(q): Query<ListQuery>) -> AppResult<Json<Value>> {
    let mine = q.filter.as_deref() == Some("mine");
    let rows: Vec<Course> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COURSE_COLS} FROM training_courses c WHERE {} {} ORDER BY c.sort, c.title",
        visible_sql(),
        if mine { "AND EXISTS (SELECT 1 FROM training_enrolments e WHERE e.course_id = c.id AND e.user_id = $1)" } else { "" }
    )))
    .bind(auth.id)
    .bind(manager(&auth))
    .fetch_all(&state.db)
    .await?;
    let mut items = Vec::new();
    for c in rows {
        let e = enrolment(&state, auth.id, c.id).await?;
        let counts: (i64, i64) = sqlx::query_as("SELECT (SELECT COUNT(*) FROM training_lessons WHERE course_id = $1), (SELECT COUNT(*) FROM training_tests WHERE course_id = $1 AND is_active)")
            .bind(c.id)
            .fetch_one(&state.db)
            .await?;
        let cert: Option<Uuid> = sqlx::query_scalar("SELECT id FROM training_certificates WHERE course_id = $1 AND user_id = $2").bind(c.id).bind(auth.id).fetch_optional(&state.db).await?;
        let excerpt: String = c.description.chars().take(240).collect();
        items.push(json!({
            "id": c.id, "title": c.title, "excerpt": excerpt, "cover_file_id": c.cover_file_id, "is_published": c.is_published,
            "lessons": counts.0, "tests": counts.1, "enrolment": e,
            "progress": if e.is_some() { progress(&state, auth.id, c.id).await? } else { Value::Null },
            "certificate_id": cert,
            "overdue": e.as_ref().is_some_and(|e| e.completed_at.is_none() && e.due_on.is_some_and(|d| d < Utc::now().date_naive())),
        }));
    }
    Ok(Json(json!({ "items": items, "can_manage": manager(&auth) })))
}

async fn get_course(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    course_view(&state, &auth, id).await.map(Json)
}

pub(crate) async fn course_view(state: &AppState, auth: &AuthUser, id: Uuid) -> AppResult<Value> {
    let c = visible_course(state, auth, id).await?;
    let e = enrolment(state, auth.id, id).await?;
    let can_enrol: bool = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT {} FROM training_courses c WHERE c.id = $2", open_to_sql())))
        .bind(auth.id)
        .bind(id)
        .fetch_one(&state.db)
        .await?;
    let chapters: Vec<(Uuid, String, i32)> = sqlx::query_as("SELECT id, title, sort FROM training_chapters WHERE course_id = $1 ORDER BY sort, title").bind(id).fetch_all(&state.db).await?;
    let lessons: Vec<(Uuid, Option<Uuid>, String, i32, i32, bool)> = sqlx::query_as(
        "SELECT l.id, l.chapter_id, l.title, l.sort, l.opens_after_days, EXISTS (SELECT 1 FROM training_lesson_progress p WHERE p.lesson_id = l.id AND p.user_id = $2)
         FROM training_lessons l LEFT JOIN training_chapters ch ON ch.id = l.chapter_id WHERE l.course_id = $1 ORDER BY ch.sort NULLS FIRST, l.sort, l.title",
    )
    .bind(id)
    .bind(auth.id)
    .fetch_all(&state.db)
    .await?;
    let now = Utc::now();
    let enrolled_at = e.as_ref().map(|e| e.enrolled_at);
    let lessons: Vec<Value> = lessons
        .into_iter()
        .map(|(lid, ch, title, sort, days, done)| {
            let at = opens_at(enrolled_at, days);
            json!({ "id": lid, "chapter_id": ch, "title": title, "sort": sort, "opens_after_days": days, "opens_at": at,
                    "open": manager(auth) || (e.is_some() && at.is_none_or(|t| t <= now)), "done": done })
        })
        .collect();
    let tests = attempts::tests_for(state, auth, id).await?;
    let cert: Option<(Uuid, i64)> = sqlx::query_as("SELECT id, number FROM training_certificates WHERE course_id = $1 AND user_id = $2").bind(id).bind(auth.id).fetch_optional(&state.db).await?;
    let enrolled: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM training_enrolments WHERE course_id = $1").bind(id).fetch_one(&state.db).await?;
    let mut v = json!({
        "course": c, "html": crate::kb::render(&c.description), "chapters": chapters.into_iter().map(|(i, t, s)| json!({ "id": i, "title": t, "sort": s })).collect::<Vec<_>>(),
        "lessons": lessons, "tests": tests, "enrolment": e, "can_enrol": can_enrol && e.is_none(),
        "progress": if e.is_some() { progress(state, auth.id, id).await? } else { Value::Null },
        "certificate": cert.map(|(i, n)| json!({ "id": i, "number": n })), "enrolled": enrolled, "can_manage": manager(auth),
    });
    if manager(auth) {
        let rows: Vec<(Option<Uuid>, Option<Uuid>, Option<String>)> = sqlx::query_as(
            "SELECT a.user_id, a.department_id, COALESCE(trim(u.first_name || ' ' || u.last_name), d.name) FROM training_access a
             LEFT JOIN users u ON u.id = a.user_id LEFT JOIN departments d ON d.id = a.department_id WHERE a.course_id = $1",
        )
        .bind(id)
        .fetch_all(&state.db)
        .await?;
        v["audience"] = json!({
            "users": rows.iter().filter(|r| r.0.is_some()).map(|r| json!({ "id": r.0, "name": r.2 })).collect::<Vec<_>>(),
            "departments": rows.iter().filter_map(|r| r.1).collect::<Vec<_>>(),
        });
    }
    Ok(v)
}

#[derive(Deserialize, Default)]
struct Who {
    #[serde(default)]
    users: Vec<Uuid>,
    #[serde(default)]
    departments: Vec<Uuid>,
}

async fn check_who(state: &AppState, w: &Who) -> AppResult<()> {
    let u: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE id = ANY($1) AND is_active").bind(&w.users).fetch_one(&state.db).await?;
    let d: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM departments WHERE id = ANY($1)").bind(&w.departments).fetch_one(&state.db).await?;
    if u as usize != w.users.len() || d as usize != w.departments.len() {
        return Err(AppError::bad("some of the people or departments don't exist"));
    }
    Ok(())
}

#[derive(Deserialize)]
struct CourseInput {
    title: Option<String>,
    description: Option<String>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    cover_file_id: Option<Option<Uuid>>,
    is_published: Option<bool>,
    to_all: Option<bool>,
    audience: Option<Who>,
    sort: Option<i32>,
}

async fn set_audience(state: &AppState, course: Uuid, w: &Who) -> AppResult<()> {
    let mut tx = state.db.begin().await?;
    sqlx::query("DELETE FROM training_access WHERE course_id = $1").bind(course).execute(&mut *tx).await?;
    for u in &w.users {
        sqlx::query("INSERT INTO training_access (course_id, user_id) VALUES ($1, $2)").bind(course).bind(u).execute(&mut *tx).await?;
    }
    for d in &w.departments {
        sqlx::query("INSERT INTO training_access (course_id, department_id) VALUES ($1, $2)").bind(course).bind(d).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(())
}

async fn create_course(State(state): State<AppState>, auth: AuthUser, Json(b): Json<CourseInput>) -> AppResult<Json<Value>> {
    require_manager(&auth)?;
    let title = required("title", b.title.as_deref().unwrap_or(""), 200)?;
    let description = b.description.unwrap_or_default();
    if description.len() > 100_000 {
        return Err(AppError::bad("the description is too long"));
    }
    let audience = b.audience.unwrap_or_default();
    check_who(&state, &audience).await?;
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO training_courses (title, description, cover_file_id, is_published, to_all, sort, created_by) VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING id",
    )
    .bind(&title)
    .bind(description.trim())
    .bind(b.cover_file_id.flatten())
    .bind(b.is_published.unwrap_or(false))
    .bind(b.to_all.unwrap_or(true))
    .bind(b.sort.unwrap_or(500))
    .bind(auth.id)
    .fetch_one(&state.db)
    .await?;
    set_audience(&state, id, &audience).await?;
    course_view(&state, &auth, id).await.map(Json)
}

async fn update_course(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<CourseInput>) -> AppResult<Json<Value>> {
    require_manager(&auth)?;
    visible_course(&state, &auth, id).await?;
    let title = match &b.title {
        Some(t) => Some(required("title", t, 200)?),
        None => None,
    };
    if b.description.as_ref().is_some_and(|d| d.len() > 100_000) {
        return Err(AppError::bad("the description is too long"));
    }
    sqlx::query(
        "UPDATE training_courses SET title = COALESCE($2, title), description = COALESCE($3, description),
            cover_file_id = CASE WHEN $4 THEN $5 ELSE cover_file_id END, is_published = COALESCE($6, is_published),
            to_all = COALESCE($7, to_all), sort = COALESCE($8, sort), updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(&title)
    .bind(b.description.as_deref().map(str::trim))
    .bind(b.cover_file_id.is_some())
    .bind(b.cover_file_id.flatten())
    .bind(b.is_published)
    .bind(b.to_all)
    .bind(b.sort)
    .execute(&state.db)
    .await?;
    if let Some(w) = &b.audience {
        check_who(&state, w).await?;
        set_audience(&state, id, w).await?;
    }
    course_view(&state, &auth, id).await.map(Json)
}

async fn delete_course(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    require_manager(&auth)?;
    let c = visible_course(&state, &auth, id).await?;
    sqlx::query("DELETE FROM training_courses WHERE id = $1").bind(id).execute(&state.db).await?;
    crate::audit::record(&state.db, Some(auth.id), "training.course_delete", "training_course", Some(id.to_string()), json!({ "title": c.title }), None).await;
    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// Enrolling and assigning
// ---------------------------------------------------------------------------

async fn enrol(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let open: Option<bool> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT {} FROM training_courses c WHERE c.id = $2", open_to_sql())))
        .bind(auth.id)
        .bind(id)
        .fetch_optional(&state.db)
        .await?;
    match open {
        None => return Err(AppError::NotFound("course")),
        Some(false) => {
            visible_course(&state, &auth, id).await?;
            return Err(AppError::bad("this course isn't open for enrolling"));
        }
        Some(true) => {}
    }
    sqlx::query("INSERT INTO training_enrolments (course_id, user_id) VALUES ($1, $2) ON CONFLICT DO NOTHING").bind(id).bind(auth.id).execute(&state.db).await?;
    check_completion(&state, auth.id, id).await?;
    course_view(&state, &auth, id).await.map(Json)
}

/// Leave a course you enrolled in yourself (an assigned course stays until it is unassigned).
async fn leave(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let e = enrolment(&state, auth.id, id).await?.ok_or(AppError::NotFound("enrolment"))?;
    if e.source == "assigned" {
        return Err(AppError::bad("this course was assigned to you"));
    }
    sqlx::query("DELETE FROM training_enrolments WHERE course_id = $1 AND user_id = $2").bind(id).bind(auth.id).execute(&state.db).await?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct AssignInput {
    #[serde(default)]
    users: Vec<Uuid>,
    #[serde(default)]
    departments: Vec<Uuid>,
    due_on: Option<NaiveDate>,
}

async fn assign(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<AssignInput>) -> AppResult<Json<Value>> {
    require_manager(&auth)?;
    let c = visible_course(&state, &auth, id).await?;
    if !c.is_published {
        return Err(AppError::bad("publish the course before assigning it"));
    }
    let w = Who { users: b.users, departments: b.departments };
    check_who(&state, &w).await?;
    if b.due_on.is_some_and(|d| d < Utc::now().date_naive()) {
        return Err(AppError::bad("the due date has passed"));
    }
    // The people, and everyone in the departments and their sub-departments.
    let people: Vec<Uuid> = sqlx::query_scalar(
        "WITH RECURSIVE tree AS (SELECT id FROM departments WHERE id = ANY($2) UNION SELECT d.id FROM departments d JOIN tree t ON d.parent_id = t.id)
         SELECT DISTINCT u.id FROM users u WHERE u.is_active AND (u.id = ANY($1) OR u.id IN (SELECT user_id FROM user_departments WHERE department_id IN (SELECT id FROM tree)))",
    )
    .bind(&w.users)
    .bind(&w.departments)
    .fetch_all(&state.db)
    .await?;
    if people.is_empty() {
        return Err(AppError::bad("choose people or departments"));
    }
    let mut newly = Vec::new();
    for u in &people {
        // Skipped for anyone who already completed the course.
        let fresh: Option<Uuid> = sqlx::query_scalar(
            "INSERT INTO training_enrolments (course_id, user_id, source, assigned_by, due_on) VALUES ($1, $2, 'assigned', $3, $4)
             ON CONFLICT (course_id, user_id) DO UPDATE SET source = 'assigned', assigned_by = $3, due_on = $4, reminded_at = NULL
             WHERE training_enrolments.completed_at IS NULL
             RETURNING id",
        )
        .bind(id)
        .bind(u)
        .bind(auth.id)
        .bind(b.due_on)
        .fetch_optional(&state.db)
        .await?;
        if fresh.is_some() {
            newly.push(*u);
        }
    }
    notify(&state, &newly, NewNotification {
        kind: "training.assigned",
        title: format!("Training assigned: {}", c.title),
        body: b.due_on.map(|d| format!("Due {}", d.format("%-d %B %Y"))),
        link: Some(format!("/training/courses/{id}")),
        actor: Some(auth.id),
    })
    .await;
    Ok(Json(json!({ "assigned": newly.len(), "already_completed": people.len() - newly.len() })))
}

async fn unenrol(State(state): State<AppState>, auth: AuthUser, Path((id, user)): Path<(Uuid, Uuid)>) -> AppResult<Json<Value>> {
    require_manager(&auth)?;
    let r = sqlx::query("DELETE FROM training_enrolments WHERE course_id = $1 AND user_id = $2").bind(id).bind(user).execute(&state.db).await?;
    if r.rows_affected() == 0 {
        return Err(AppError::NotFound("enrolment"));
    }
    Ok(Json(json!({ "ok": true })))
}

/// Reminds people of assigned training due within two days (once).
pub async fn run_reminders(state: &AppState) -> Result<()> {
    let due: Vec<(Uuid, Uuid, String, NaiveDate)> = sqlx::query_as(
        "UPDATE training_enrolments e SET reminded_at = now() FROM training_courses c
         WHERE c.id = e.course_id AND e.completed_at IS NULL AND e.reminded_at IS NULL AND e.due_on IS NOT NULL
           AND e.due_on <= CURRENT_DATE + 2 AND e.due_on >= CURRENT_DATE
         RETURNING e.user_id, c.id, c.title, e.due_on",
    )
    .fetch_all(&state.db)
    .await?;
    for (user, course, title, d) in due {
        notify(state, &[user], NewNotification {
            kind: "training.due",
            title: format!("Training due {}: {title}", d.format("%-d %B")),
            body: None,
            link: Some(format!("/training/courses/{course}")),
            actor: None,
        })
        .await;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Chapters and lessons
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct ChapterInput {
    title: Option<String>,
    sort: Option<i32>,
}

async fn create_chapter(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<ChapterInput>) -> AppResult<Json<Value>> {
    require_manager(&auth)?;
    visible_course(&state, &auth, id).await?;
    let title = required("title", b.title.as_deref().unwrap_or(""), 200)?;
    sqlx::query("INSERT INTO training_chapters (course_id, title, sort) VALUES ($1, $2, COALESCE($3, (SELECT COALESCE(MAX(sort), 0) + 10 FROM training_chapters WHERE course_id = $1)))")
        .bind(id)
        .bind(&title)
        .bind(b.sort)
        .execute(&state.db)
        .await?;
    course_view(&state, &auth, id).await.map(Json)
}

async fn chapter_course(state: &AppState, id: Uuid) -> AppResult<Uuid> {
    sqlx::query_scalar("SELECT course_id FROM training_chapters WHERE id = $1").bind(id).fetch_optional(&state.db).await?.ok_or(AppError::NotFound("chapter"))
}

async fn update_chapter(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<ChapterInput>) -> AppResult<Json<Value>> {
    require_manager(&auth)?;
    let course = chapter_course(&state, id).await?;
    let title = match &b.title {
        Some(t) => Some(required("title", t, 200)?),
        None => None,
    };
    sqlx::query("UPDATE training_chapters SET title = COALESCE($2, title), sort = COALESCE($3, sort) WHERE id = $1").bind(id).bind(title).bind(b.sort).execute(&state.db).await?;
    course_view(&state, &auth, course).await.map(Json)
}

/// Its lessons stay, outside any chapter.
async fn delete_chapter(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    require_manager(&auth)?;
    let course = chapter_course(&state, id).await?;
    sqlx::query("DELETE FROM training_chapters WHERE id = $1").bind(id).execute(&state.db).await?;
    course_view(&state, &auth, course).await.map(Json)
}

#[derive(Deserialize)]
struct LessonInput {
    title: Option<String>,
    body: Option<String>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    chapter_id: Option<Option<Uuid>>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    video_url: Option<Option<String>>,
    file_ids: Option<Vec<Uuid>>,
    opens_after_days: Option<i32>,
    sort: Option<i32>,
}

async fn check_lesson_input(state: &AppState, course: Uuid, b: &LessonInput) -> AppResult<Option<Option<String>>> {
    if let Some(Some(ch)) = b.chapter_id {
        if chapter_course(state, ch).await? != course {
            return Err(AppError::bad("that chapter is in another course"));
        }
    }
    if b.body.as_ref().is_some_and(|x| x.len() > 200_000) {
        return Err(AppError::bad("the lesson is too long"));
    }
    if b.opens_after_days.is_some_and(|d| !(0..=3650).contains(&d)) {
        return Err(AppError::bad("opens after 0 to 3650 days"));
    }
    let video = match &b.video_url {
        Some(v) => {
            let v = clean(v.clone());
            if v.as_ref().is_some_and(|u| !(u.starts_with("https://") || u.starts_with("http://")) || u.len() > 500) {
                return Err(AppError::bad("the video link must be a web address"));
            }
            Some(v)
        }
        None => None,
    };
    if let Some(ids) = &b.file_ids {
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM files WHERE id = ANY($1)").bind(ids).fetch_one(&state.db).await?;
        if n as usize != ids.len() || ids.len() > 30 {
            return Err(AppError::bad("some files don't exist"));
        }
    }
    Ok(video)
}

async fn create_lesson(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<LessonInput>) -> AppResult<Json<Value>> {
    require_manager(&auth)?;
    visible_course(&state, &auth, id).await?;
    let title = required("title", b.title.as_deref().unwrap_or(""), 200)?;
    let video = check_lesson_input(&state, id, &b).await?;
    let lid: Uuid = sqlx::query_scalar(
        "INSERT INTO training_lessons (course_id, chapter_id, title, body, video_url, file_ids, opens_after_days, sort)
         VALUES ($1, $2, $3, $4, $5, $6, $7, COALESCE($8, (SELECT COALESCE(MAX(sort), 0) + 10 FROM training_lessons WHERE course_id = $1))) RETURNING id",
    )
    .bind(id)
    .bind(b.chapter_id.flatten())
    .bind(&title)
    .bind(b.body.as_deref().unwrap_or("").trim())
    .bind(video.flatten())
    .bind(b.file_ids.unwrap_or_default())
    .bind(b.opens_after_days.unwrap_or(0))
    .bind(b.sort)
    .fetch_one(&state.db)
    .await?;
    lesson_view(&state, &auth, lid).await.map(Json)
}

async fn lesson_course(state: &AppState, id: Uuid) -> AppResult<Uuid> {
    sqlx::query_scalar("SELECT course_id FROM training_lessons WHERE id = $1").bind(id).fetch_optional(&state.db).await?.ok_or(AppError::NotFound("lesson"))
}

#[derive(FromRow)]
struct LessonRow {
    id: Uuid,
    course_id: Uuid,
    chapter_id: Option<Uuid>,
    title: String,
    body: String,
    video_url: Option<String>,
    file_ids: Vec<Uuid>,
    opens_after_days: i32,
    sort: i32,
}

async fn lesson_view(state: &AppState, auth: &AuthUser, id: Uuid) -> AppResult<Value> {
    let l: LessonRow = sqlx::query_as("SELECT id, course_id, chapter_id, title, body, video_url, file_ids, opens_after_days, sort FROM training_lessons WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound("lesson"))?;
    let c = visible_course(state, auth, l.course_id).await?;
    let e = require_enrolled(state, auth, l.course_id).await?;
    if !manager(auth) {
        if let Some(at) = opens_at(e.as_ref().map(|e| e.enrolled_at), l.opens_after_days) {
            if at > Utc::now() {
                return Err(AppError::bad(format!("this lesson opens on {}", at.format("%-d %B %Y"))));
            }
        }
    }
    let files: Vec<(Uuid, String, i64, String)> = sqlx::query_as("SELECT id, original_name, size_bytes, mime_type FROM files WHERE id = ANY($1)").bind(&l.file_ids).fetch_all(&state.db).await?;
    let done: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM training_lesson_progress WHERE lesson_id = $1 AND user_id = $2)").bind(id).bind(auth.id).fetch_one(&state.db).await?;
    // The lessons in course order, for "previous" and "next".
    let order: Vec<Uuid> = sqlx::query_scalar(
        "SELECT l.id FROM training_lessons l LEFT JOIN training_chapters ch ON ch.id = l.chapter_id WHERE l.course_id = $1 ORDER BY ch.sort NULLS FIRST, l.sort, l.title",
    )
    .bind(l.course_id)
    .fetch_all(&state.db)
    .await?;
    let at = order.iter().position(|x| *x == id).unwrap_or(0);
    let checks = content::self_checks(state, id, manager(auth)).await?;
    Ok(json!({
        "id": l.id, "course": { "id": c.id, "title": c.title }, "chapter_id": l.chapter_id, "title": l.title, "body": l.body,
        "html": crate::kb::render(&l.body), "video_url": l.video_url, "opens_after_days": l.opens_after_days, "sort": l.sort,
        "files": files.into_iter().map(|(i, n, s, m)| json!({ "id": i, "name": n, "size": s, "mime_type": m })).collect::<Vec<_>>(),
        "done": done, "enrolled": e.is_some(), "self_checks": checks,
        "previous": if at > 0 { order.get(at - 1).copied() } else { None }, "next": order.get(at + 1).copied(),
        "can_manage": manager(auth),
    }))
}

async fn get_lesson(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    lesson_view(&state, &auth, id).await.map(Json)
}

async fn update_lesson(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<LessonInput>) -> AppResult<Json<Value>> {
    require_manager(&auth)?;
    let course = lesson_course(&state, id).await?;
    let title = match &b.title {
        Some(t) => Some(required("title", t, 200)?),
        None => None,
    };
    let video = check_lesson_input(&state, course, &b).await?;
    sqlx::query(
        "UPDATE training_lessons SET title = COALESCE($2, title), body = COALESCE($3, body),
            chapter_id = CASE WHEN $4 THEN $5 ELSE chapter_id END, video_url = CASE WHEN $6 THEN $7 ELSE video_url END,
            file_ids = COALESCE($8, file_ids), opens_after_days = COALESCE($9, opens_after_days), sort = COALESCE($10, sort), updated_at = now()
         WHERE id = $1",
    )
    .bind(id)
    .bind(title)
    .bind(b.body.as_deref().map(str::trim))
    .bind(b.chapter_id.is_some())
    .bind(b.chapter_id.flatten())
    .bind(video.is_some())
    .bind(video.flatten())
    .bind(b.file_ids)
    .bind(b.opens_after_days)
    .bind(b.sort)
    .execute(&state.db)
    .await?;
    lesson_view(&state, &auth, id).await.map(Json)
}

async fn delete_lesson(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    require_manager(&auth)?;
    let course = lesson_course(&state, id).await?;
    sqlx::query("DELETE FROM training_lessons WHERE id = $1").bind(id).execute(&state.db).await?;
    course_view(&state, &auth, course).await.map(Json)
}

async fn complete_lesson(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    // Checks access, enrolment and that the lesson is open.
    let v = lesson_view(&state, &auth, id).await?;
    if v["enrolled"] != json!(true) {
        return Err(AppError::bad("enrol in the course first"));
    }
    let course = lesson_course(&state, id).await?;
    sqlx::query("INSERT INTO training_lesson_progress (lesson_id, user_id) VALUES ($1, $2) ON CONFLICT DO NOTHING").bind(id).bind(auth.id).execute(&state.db).await?;
    check_completion(&state, auth.id, course).await?;
    Ok(Json(json!({ "done": true, "progress": progress(&state, auth.id, course).await?, "next": v["next"] })))
}

async fn uncomplete_lesson(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let course = lesson_course(&state, id).await?;
    visible_course(&state, &auth, course).await?;
    sqlx::query("DELETE FROM training_lesson_progress WHERE lesson_id = $1 AND user_id = $2").bind(id).bind(auth.id).execute(&state.db).await?;
    Ok(Json(json!({ "done": false, "progress": progress(&state, auth.id, course).await? })))
}

// ---------------------------------------------------------------------------
// Certificates
// ---------------------------------------------------------------------------

#[derive(Serialize, FromRow)]
struct Certificate {
    id: Uuid,
    number: i64,
    course_id: Uuid,
    course_title: String,
    user_id: Uuid,
    user_name: String,
    issued_at: DateTime<Utc>,
    score: i32,
    max_score: i32,
}

const CERT_SELECT: &str = "SELECT x.id, x.number, x.course_id, c.title AS course_title, x.user_id, trim(u.first_name || ' ' || u.last_name) AS user_name,
        x.issued_at, x.score, x.max_score
    FROM training_certificates x JOIN training_courses c ON c.id = x.course_id JOIN users u ON u.id = x.user_id";

#[derive(Deserialize)]
struct CertQuery {
    user: Option<Uuid>,
    course: Option<Uuid>,
}

/// Your certificates; people who manage training see anyone's (by person or course).
async fn list_certificates(State(state): State<AppState>, auth: AuthUser, Query(q): Query<CertQuery>) -> AppResult<Json<Value>> {
    let user = if manager(&auth) { q.user } else { Some(auth.id) };
    if !manager(&auth) && q.user.is_some_and(|u| u != auth.id) {
        return Err(AppError::Forbidden);
    }
    let items: Vec<Certificate> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{CERT_SELECT} WHERE ($1::uuid IS NULL OR x.user_id = $1) AND ($2::uuid IS NULL OR x.course_id = $2) ORDER BY x.issued_at DESC LIMIT 500"
    )))
    .bind(user)
    .bind(q.course)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({ "items": items })))
}

async fn get_certificate(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let c: Certificate = sqlx::query_as(sqlx::AssertSqlSafe(format!("{CERT_SELECT} WHERE x.id = $1")))
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound("certificate"))?;
    if c.user_id != auth.id && !manager(&auth) {
        return Err(AppError::NotFound("certificate"));
    }
    let company: Option<Value> = sqlx::query_scalar("SELECT value FROM settings WHERE key = 'company_name'").fetch_optional(&state.db).await.ok().flatten();
    let company = company.and_then(|v| v.as_str().map(String::from));
    Ok(Json(json!({ "certificate": c, "company": company, "can_manage": manager(&auth) })))
}

/// Revoke (a mistake): the course counts as not completed until it is completed again.
async fn revoke_certificate(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    require_manager(&auth)?;
    let row: Option<(Uuid, Uuid)> = sqlx::query_as("DELETE FROM training_certificates WHERE id = $1 RETURNING course_id, user_id").bind(id).fetch_optional(&state.db).await?;
    let (course, user) = row.ok_or(AppError::NotFound("certificate"))?;
    sqlx::query("UPDATE training_enrolments SET completed_at = NULL WHERE course_id = $1 AND user_id = $2").bind(course).bind(user).execute(&state.db).await?;
    crate::audit::record(&state.db, Some(auth.id), "training.certificate_revoke", "user", Some(user.to_string()), json!({ "course": course }), None).await;
    Ok(Json(json!({ "ok": true })))
}
