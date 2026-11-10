//! News feed: what's going on in the company.
//!
//! A post goes to everyone, or to chosen people and departments (a department includes its
//! sub-departments). It can carry files and a poll, and be marked important: important
//! posts stay at the top until each reader confirms "I've read it", and their author sees who
//! has. People like and comment; the author is notified of comments, people in the audience
//! of important posts. Authors edit and delete their posts; people who manage settings can
//! delete any post or comment.

use axum::{
    extract::{Path, Query, State},
    routing::{delete, get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::FromRow;
use uuid::Uuid;

use crate::{
    auth::{permissions as perm, AuthUser},
    error::{AppError, AppResult},
    notify::{notify, NewNotification},
    routes::required,
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/feed", get(list).post(create))
        .route("/feed/important", get(important))
        .route("/feed/{id}", get(get_post).patch(update).delete(remove))
        .route("/feed/{id}/comments", get(comments).post(comment))
        .route("/feed/comments/{id}", delete(remove_comment))
        .route("/feed/{id}/like", post(like).delete(unlike))
        .route("/feed/{id}/read", post(read))
        .route("/feed/{id}/vote", post(vote))
}

/// SQL: can user $1 see post p? Workgroup posts: its members, and everyone for an open group
/// (only them: someone who left the group no longer sees even their own posts there).
const VISIBLE: &str = "((p.project_id IS NULL AND (p.to_all OR p.author_id = $1 OR EXISTS (
    SELECT 1 FROM feed_audience a WHERE a.post_id = p.id AND (a.user_id = $1 OR a.department_id IN (
        WITH RECURSIVE mine AS (
            SELECT department_id AS id FROM user_departments WHERE user_id = $1
            UNION SELECT d.parent_id FROM departments d JOIN mine m ON d.id = m.id WHERE d.parent_id IS NOT NULL
        ) SELECT id FROM mine)))))
    OR EXISTS (SELECT 1 FROM projects g WHERE g.id = p.project_id AND (g.visibility = 'open'
        OR EXISTS (SELECT 1 FROM project_members gm WHERE gm.project_id = g.id AND gm.user_id = $1))))";

/// SQL: in user $1's main feed, workgroup posts are those of their own groups.
const IN_MY_FEED: &str = "(p.project_id IS NULL OR p.author_id = $1 OR EXISTS (SELECT 1 FROM project_members gm WHERE gm.project_id = p.project_id AND gm.user_id = $1))";

fn moderator(auth: &AuthUser) -> bool {
    auth.can(perm::SETTINGS_MANAGE)
}

/// Files on posts are readable by whoever can see the post.
pub async fn can_view_file(state: &AppState, auth: &AuthUser, file: Uuid) -> AppResult<bool> {
    Ok(sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT EXISTS (SELECT 1 FROM feed_posts p WHERE $2 = ANY(p.file_ids) AND {VISIBLE})"))).bind(auth.id).bind(file).fetch_one(&state.db).await?)
}

/// Everyone a post goes to (for notifications and live updates).
async fn audience(state: &AppState, post: Uuid) -> AppResult<Vec<Uuid>> {
    Ok(sqlx::query_scalar(
        "WITH RECURSIVE p AS (SELECT * FROM feed_posts WHERE id = $1),
         depts AS (
             SELECT a.department_id AS id FROM feed_audience a WHERE a.post_id = $1 AND a.department_id IS NOT NULL
             UNION SELECT d.id FROM departments d JOIN depts x ON d.parent_id = x.id
         )
         SELECT u.id FROM users u, p WHERE u.is_active AND (
             p.to_all OR u.id = p.author_id
             OR u.id IN (SELECT user_id FROM feed_audience WHERE post_id = $1 AND user_id IS NOT NULL)
             OR u.id IN (SELECT user_id FROM project_members WHERE project_id = p.project_id)
             OR u.id IN (SELECT user_id FROM user_departments WHERE department_id IN (SELECT id FROM depts)))",
    )
    .bind(post)
    .fetch_all(&state.db)
    .await?)
}

#[derive(Serialize, FromRow)]
struct PostRow {
    id: Uuid,
    author_id: Option<Uuid>,
    author_name: Option<String>,
    author_avatar: Option<Uuid>,
    body: String,
    files: Value,
    to_all: bool,
    important: bool,
    poll: Option<Value>,
    created_at: DateTime<Utc>,
    edited_at: Option<DateTime<Utc>>,
    likes: i64,
    liked: bool,
    comments: i64,
    /// Important posts: whether I confirmed, and how many have.
    read_by_me: bool,
    reads: i64,
    /// Polls: votes per option, and mine.
    votes: Value,
    my_vote: Option<i32>,
    /// Names of the people and departments it went to (empty: everyone).
    to: Value,
    /// A workgroup's post.
    project_id: Option<Uuid>,
    project_name: Option<String>,
}

const POST_SELECT: &str = "SELECT p.id, p.author_id, trim(u.first_name || ' ' || u.last_name) AS author_name, u.avatar_file_id AS author_avatar, p.body,
    COALESCE((SELECT jsonb_agg(jsonb_build_object('id', f.id, 'name', f.original_name, 'size', f.size_bytes, 'mime', f.mime_type)) FROM files f WHERE f.id = ANY(p.file_ids)), '[]') AS files,
    p.to_all, p.important, p.poll, p.created_at, p.edited_at,
    (SELECT COUNT(*) FROM feed_likes l WHERE l.post_id = p.id) AS likes,
    EXISTS (SELECT 1 FROM feed_likes l WHERE l.post_id = p.id AND l.user_id = $1) AS liked,
    (SELECT COUNT(*) FROM feed_comments c WHERE c.post_id = p.id) AS comments,
    EXISTS (SELECT 1 FROM feed_reads r WHERE r.post_id = p.id AND r.user_id = $1) AS read_by_me,
    (SELECT COUNT(*) FROM feed_reads r WHERE r.post_id = p.id) AS reads,
    COALESCE((SELECT jsonb_object_agg(v.option, v.n) FROM (SELECT option, COUNT(*) AS n FROM feed_votes WHERE post_id = p.id GROUP BY option) v), '{}') AS votes,
    (SELECT option FROM feed_votes v WHERE v.post_id = p.id AND v.user_id = $1) AS my_vote,
    COALESCE((SELECT jsonb_agg(COALESCE(trim(au.first_name || ' ' || au.last_name), d.name)) FROM feed_audience a
        LEFT JOIN users au ON au.id = a.user_id LEFT JOIN departments d ON d.id = a.department_id WHERE a.post_id = p.id), '[]') AS \"to\",
    p.project_id, (SELECT g.name FROM projects g WHERE g.id = p.project_id) AS project_name
    FROM feed_posts p LEFT JOIN users u ON u.id = p.author_id";

#[derive(Deserialize)]
struct ListQuery {
    before: Option<DateTime<Utc>>,
    limit: Option<i64>,
    q: Option<String>,
    /// A workgroup's feed.
    project_id: Option<Uuid>,
}

async fn list(State(state): State<AppState>, auth: AuthUser, Query(q): Query<ListQuery>) -> AppResult<Json<Value>> {
    let limit = q.limit.unwrap_or(20).clamp(1, 100);
    let term = q.q.map(|t| t.trim().to_lowercase()).filter(|t| !t.is_empty()).map(|t| format!("%{}%", t.replace('%', "\\%").replace('_', "\\_")));
    if let Some(g) = q.project_id {
        crate::tasks::access::project_access(&state, &auth, g).await?;
    }
    let mut items: Vec<PostRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{POST_SELECT} WHERE {VISIBLE} AND ($2::timestamptz IS NULL OR p.created_at < $2) AND ($3::text IS NULL OR lower(p.body) LIKE $3)
           AND (CASE WHEN $5::uuid IS NULL THEN {IN_MY_FEED} ELSE p.project_id = $5 END)
         ORDER BY p.created_at DESC LIMIT $4"
    )))
    .bind(auth.id)
    .bind(q.before)
    .bind(term)
    .bind(limit + 1)
    .bind(q.project_id)
    .fetch_all(&state.db)
    .await?;
    let more = items.len() as i64 > limit;
    items.truncate(limit as usize);
    Ok(Json(json!({ "items": items, "more": more })))
}

/// Important posts I haven't confirmed yet (shown at the top).
async fn important(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    let items: Vec<PostRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{POST_SELECT} WHERE {VISIBLE} AND {IN_MY_FEED} AND p.important AND p.author_id IS DISTINCT FROM $1
           AND NOT EXISTS (SELECT 1 FROM feed_reads r WHERE r.post_id = p.id AND r.user_id = $1)
         ORDER BY p.created_at DESC LIMIT 20"
    )))
    .bind(auth.id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({ "items": items })))
}

async fn post_row(state: &AppState, auth: &AuthUser, id: Uuid) -> AppResult<PostRow> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{POST_SELECT} WHERE {VISIBLE} AND p.id = $2")))
        .bind(auth.id)
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound("post"))
}

async fn get_post(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let p = post_row(&state, &auth, id).await?;
    let mut v = serde_json::to_value(&p).unwrap();
    // Who has confirmed an important post: for its author.
    if p.important && p.author_id == Some(auth.id) {
        let who: Vec<(Uuid, String, Option<DateTime<Utc>>)> = sqlx::query_as(
            "SELECT u.id, trim(u.first_name || ' ' || u.last_name), r.read_at FROM users u LEFT JOIN feed_reads r ON r.user_id = u.id AND r.post_id = $1
             WHERE u.id = ANY($2) AND u.id <> $3 ORDER BY r.read_at NULLS LAST, u.first_name",
        )
        .bind(id)
        .bind(audience(&state, id).await?)
        .bind(auth.id)
        .fetch_all(&state.db)
        .await?;
        v["readers"] = json!(who.into_iter().map(|(id, name, at)| json!({ "id": id, "name": name, "read_at": at })).collect::<Vec<_>>());
    }
    Ok(Json(v))
}

#[derive(Deserialize)]
struct PollInput {
    question: String,
    options: Vec<String>,
}

#[derive(Deserialize)]
struct PostInput {
    body: Option<String>,
    #[serde(default)]
    file_ids: Vec<Uuid>,
    #[serde(default = "yes")]
    to_all: bool,
    #[serde(default)]
    users: Vec<Uuid>,
    #[serde(default)]
    departments: Vec<Uuid>,
    #[serde(default)]
    important: bool,
    poll: Option<PollInput>,
    /// Post in a workgroup's feed (to its members).
    project_id: Option<Uuid>,
}
fn yes() -> bool {
    true
}

fn clean_poll(p: Option<PollInput>) -> AppResult<Option<Value>> {
    let Some(p) = p else { return Ok(None) };
    let question = required("poll question", &p.question, 300)?;
    let options: Vec<String> = p.options.iter().map(|o| o.trim().to_string()).filter(|o| !o.is_empty()).collect();
    if options.len() < 2 || options.len() > 10 {
        return Err(AppError::bad("a poll has two to ten answers"));
    }
    if options.iter().any(|o| o.chars().count() > 200) {
        return Err(AppError::bad("answers can be at most 200 characters"));
    }
    Ok(Some(json!({ "question": question, "options": options })))
}

/// Posts to the feed from the same JSON the API takes (for other modules, e.g. sharing an email).
pub async fn create_post(state: &AppState, auth: &AuthUser, input: Value) -> AppResult<Value> {
    let b: PostInput = serde_json::from_value(input).map_err(|e| AppError::bad(format!("post: {e}")))?;
    create(State(state.clone()), auth.clone(), Json(b)).await.map(|j| j.0)
}

async fn create(State(state): State<AppState>, auth: AuthUser, Json(b): Json<PostInput>) -> AppResult<Json<Value>> {
    let body = required("post", b.body.as_deref().unwrap_or(""), 20_000)?;
    if b.file_ids.len() > 20 {
        return Err(AppError::bad("at most 20 files"));
    }
    let own: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM files WHERE id = ANY($1) AND owner_id = $2").bind(&b.file_ids).bind(auth.id).fetch_one(&state.db).await?;
    if own as usize != b.file_ids.len() {
        return Err(AppError::bad("attach files you uploaded"));
    }
    let mut b = b;
    let poll = clean_poll(b.poll.take())?;
    if let Some(g) = b.project_id {
        if !crate::tasks::access::project_access(&state, &auth, g).await?.contribute {
            return Err(AppError::Forbidden);
        }
        b.to_all = false;
        b.users.clear();
        b.departments.clear();
    } else if !b.to_all {
        if b.users.is_empty() && b.departments.is_empty() {
            return Err(AppError::bad("choose who the post is for"));
        }
        let users: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE id = ANY($1) AND is_active").bind(&b.users).fetch_one(&state.db).await?;
        let depts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM departments WHERE id = ANY($1)").bind(&b.departments).fetch_one(&state.db).await?;
        if users as usize != b.users.len() || depts as usize != b.departments.len() {
            return Err(AppError::bad("some of the people or departments don't exist"));
        }
    }
    let mut tx = state.db.begin().await?;
    let id: Uuid = sqlx::query_scalar("INSERT INTO feed_posts (author_id, body, file_ids, to_all, important, poll, project_id) VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING id")
        .bind(auth.id)
        .bind(&body)
        .bind(&b.file_ids)
        .bind(b.to_all)
        .bind(b.important)
        .bind(poll)
        .bind(b.project_id)
        .fetch_one(&mut *tx)
        .await?;
    if !b.to_all {
        for u in &b.users {
            sqlx::query("INSERT INTO feed_audience (post_id, user_id) VALUES ($1, $2)").bind(id).bind(u).execute(&mut *tx).await?;
        }
        for d in &b.departments {
            sqlx::query("INSERT INTO feed_audience (post_id, department_id) VALUES ($1, $2)").bind(id).bind(d).execute(&mut *tx).await?;
        }
    }
    tx.commit().await?;
    let who = audience(&state, id).await?;
    let name: String = sqlx::query_scalar("SELECT trim(first_name || ' ' || last_name) FROM users WHERE id = $1").bind(auth.id).fetch_one(&state.db).await?;
    if b.important {
        notify(&state, &who, NewNotification {
            kind: "feed.important",
            title: format!("Important from {name}: {}", body.chars().take(80).collect::<String>()),
            body: None,
            link: Some(format!("/feed?post={id}")),
            actor: Some(auth.id),
        })
        .await;
    }
    crate::realtime::publish(&state, Some(who), "feed.post", json!({ "post_id": id })).await;
    get_post(State(state), auth, Path(id)).await
}

#[derive(Deserialize)]
struct PostPatch {
    body: Option<String>,
}

async fn update(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<PostPatch>) -> AppResult<Json<Value>> {
    let p = post_row(&state, &auth, id).await?;
    if p.author_id != Some(auth.id) {
        return Err(AppError::Forbidden);
    }
    if let Some(body) = b.body {
        let body = required("post", &body, 20_000)?;
        sqlx::query("UPDATE feed_posts SET body = $2, edited_at = now() WHERE id = $1").bind(id).bind(body).execute(&state.db).await?;
    }
    get_post(State(state), auth, Path(id)).await
}

async fn remove(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let p = post_row(&state, &auth, id).await?;
    // A workgroup's owners moderate its feed.
    let group_owner = match p.project_id {
        Some(g) => crate::tasks::access::project_access(&state, &auth, g).await.is_ok_and(|a| a.manage),
        None => false,
    };
    if p.author_id != Some(auth.id) && !moderator(&auth) && !group_owner {
        return Err(AppError::Forbidden);
    }
    sqlx::query("DELETE FROM feed_posts WHERE id = $1").bind(id).execute(&state.db).await?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Serialize, FromRow)]
struct CommentRow {
    id: Uuid,
    author_id: Option<Uuid>,
    author_name: Option<String>,
    author_avatar: Option<Uuid>,
    body: String,
    created_at: DateTime<Utc>,
}

async fn comments(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    post_row(&state, &auth, id).await?;
    let items: Vec<CommentRow> = sqlx::query_as(
        "SELECT c.id, c.author_id, trim(u.first_name || ' ' || u.last_name) AS author_name, u.avatar_file_id AS author_avatar, c.body, c.created_at
         FROM feed_comments c LEFT JOIN users u ON u.id = c.author_id WHERE c.post_id = $1 ORDER BY c.created_at LIMIT 500",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({ "items": items })))
}

#[derive(Deserialize)]
struct CommentBody {
    body: String,
}

async fn comment(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<CommentBody>) -> AppResult<Json<Value>> {
    let p = post_row(&state, &auth, id).await?;
    let body = required("comment", &b.body, 5000)?;
    sqlx::query("INSERT INTO feed_comments (post_id, author_id, body) VALUES ($1, $2, $3)").bind(id).bind(auth.id).bind(&body).execute(&state.db).await?;
    // The author, and others who commented, hear about it.
    let mut who: Vec<Uuid> = sqlx::query_scalar("SELECT DISTINCT author_id FROM feed_comments WHERE post_id = $1 AND author_id IS NOT NULL").bind(id).fetch_all(&state.db).await?;
    who.extend(p.author_id);
    who.sort();
    who.dedup();
    let name: String = sqlx::query_scalar("SELECT first_name FROM users WHERE id = $1").bind(auth.id).fetch_one(&state.db).await?;
    notify(&state, &who, NewNotification {
        kind: "feed.comment",
        title: format!("{name} commented: {}", body.chars().take(80).collect::<String>()),
        body: Some(p.body.chars().take(120).collect()),
        link: Some(format!("/feed?post={id}")),
        actor: Some(auth.id),
    })
    .await;
    crate::realtime::publish(&state, Some(audience(&state, id).await?), "feed.changed", json!({ "post_id": id })).await;
    comments(State(state), auth, Path(id)).await
}

async fn remove_comment(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let (post, author): (Uuid, Option<Uuid>) = sqlx::query_as("SELECT post_id, author_id FROM feed_comments WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound("comment"))?;
    post_row(&state, &auth, post).await?;
    if author != Some(auth.id) && !moderator(&auth) {
        return Err(AppError::Forbidden);
    }
    sqlx::query("DELETE FROM feed_comments WHERE id = $1").bind(id).execute(&state.db).await?;
    comments(State(state), auth, Path(post)).await
}

async fn like(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    post_row(&state, &auth, id).await?;
    sqlx::query("INSERT INTO feed_likes (post_id, user_id) VALUES ($1, $2) ON CONFLICT DO NOTHING").bind(id).bind(auth.id).execute(&state.db).await?;
    get_post(State(state), auth, Path(id)).await
}

async fn unlike(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    post_row(&state, &auth, id).await?;
    sqlx::query("DELETE FROM feed_likes WHERE post_id = $1 AND user_id = $2").bind(id).bind(auth.id).execute(&state.db).await?;
    get_post(State(state), auth, Path(id)).await
}

async fn read(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let p = post_row(&state, &auth, id).await?;
    if !p.important {
        return Err(AppError::bad("only important posts are confirmed"));
    }
    sqlx::query("INSERT INTO feed_reads (post_id, user_id) VALUES ($1, $2) ON CONFLICT DO NOTHING").bind(id).bind(auth.id).execute(&state.db).await?;
    get_post(State(state), auth, Path(id)).await
}

#[derive(Deserialize)]
struct VoteBody {
    option: i32,
}

/// One vote per person; voting again changes it.
async fn vote(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<VoteBody>) -> AppResult<Json<Value>> {
    let p = post_row(&state, &auth, id).await?;
    let n = p.poll.as_ref().and_then(|x| x["options"].as_array()).map(|a| a.len()).ok_or_else(|| AppError::bad("this post has no poll"))?;
    if b.option < 0 || b.option as usize >= n {
        return Err(AppError::bad("no such answer"));
    }
    sqlx::query("INSERT INTO feed_votes (post_id, user_id, option) VALUES ($1, $2, $3) ON CONFLICT (post_id, user_id) DO UPDATE SET option = $3")
        .bind(id)
        .bind(auth.id)
        .bind(b.option)
        .execute(&state.db)
        .await?;
    get_post(State(state), auth, Path(id)).await
}
