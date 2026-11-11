use axum::{
    extract::{Path, Query, State},
    routing::{get, post, put},
    Json, Router,
};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::{FromRow, Postgres, QueryBuilder};
use std::collections::HashMap;
use uuid::Uuid;

use super::{clean, double_option, normalize_email, required, Page, PageParams};
use super::{auth::send_invitation, security::{disable_two_factor, sessions_for, SessionInfo}};
use crate::{
    audit,
    notify::{notify, NewNotification},
    auth::{invalidate_authz, password, permissions as perm, tokens, AuthUser, ClientIp},
    error::{AppError, AppResult},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/users", get(list_users).post(create_user))
        .route("/users/export.csv", get(export_users))
        .route("/users/{id}", get(get_user).patch(update_user))
        .route("/users/{id}/roles", put(set_roles))
        .route("/users/{id}/password", post(reset_password))
        .route("/users/{id}/invite", post(resend_invite))
        .route("/users/{id}/2fa/reset", post(reset_two_factor))
        .route("/users/{id}/sessions", get(user_sessions))
        .route("/users/{id}/sessions/revoke", post(revoke_user_sessions))
        .route("/profile", axum::routing::patch(update_profile))
}

#[derive(Debug, FromRow)]
struct UserRow {
    id: Uuid,
    email: String,
    first_name: String,
    last_name: String,
    middle_name: Option<String>,
    position: Option<String>,
    phone: Option<String>,
    mobile: Option<String>,
    birthday: Option<NaiveDate>,
    gender: Option<String>,
    about: Option<String>,
    avatar_file_id: Option<Uuid>,
    timezone: String,
    locale: String,
    is_active: bool,
    invite_pending: bool,
    invited_at: Option<DateTime<Utc>>,
    two_factor_enabled: bool,
    last_login_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    last_seen_at: Option<DateTime<Utc>>,
    is_guest: bool,
}

const USER_COLUMNS: &str = "u.id, u.email, u.first_name, u.last_name, u.middle_name, u.position, \
    u.phone, u.mobile, u.birthday, u.gender, u.about, u.avatar_file_id, u.timezone, u.locale, \
    u.is_active, u.invite_pending, u.invited_at, (u.totp_secret IS NOT NULL) AS two_factor_enabled, \
    u.last_login_at, u.created_at, u.last_seen_at, u.is_guest";

#[derive(Debug, Serialize, Clone, FromRow)]
pub struct Ref {
    pub id: Uuid,
    pub name: String,
}

#[derive(Debug, Serialize, Clone, FromRow)]
pub struct RoleRef {
    pub id: Uuid,
    pub code: String,
    pub name: String,
}

#[derive(Debug, Serialize)]
pub struct User {
    pub id: Uuid,
    pub email: String,
    pub first_name: String,
    pub last_name: String,
    pub middle_name: Option<String>,
    pub full_name: String,
    pub position: Option<String>,
    pub phone: Option<String>,
    pub mobile: Option<String>,
    pub birthday: Option<NaiveDate>,
    pub gender: Option<String>,
    pub about: Option<String>,
    pub avatar_file_id: Option<Uuid>,
    pub timezone: String,
    pub locale: String,
    pub is_active: bool,
    /// Invited but has not yet accepted (cannot sign in).
    pub invite_pending: bool,
    /// Someone from outside the company, in the chats and workgroups they were invited to.
    pub is_guest: bool,
    pub invited_at: Option<DateTime<Utc>>,
    pub two_factor_enabled: bool,
    pub last_login_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    /// Last active, and whether that was in the last five minutes.
    pub last_seen_at: Option<DateTime<Utc>>,
    pub online: bool,
    pub departments: Vec<Ref>,
    pub roles: Vec<RoleRef>,
}

async fn hydrate(state: &AppState, rows: Vec<UserRow>) -> AppResult<Vec<User>> {
    let ids: Vec<Uuid> = rows.iter().map(|r| r.id).collect();

    let dept_rows: Vec<(Uuid, Uuid, String)> = sqlx::query_as(
        "SELECT ud.user_id, d.id, d.name FROM user_departments ud
         JOIN departments d ON d.id = ud.department_id
         WHERE ud.user_id = ANY($1) ORDER BY d.sort, d.name",
    )
    .bind(&ids)
    .fetch_all(&state.db)
    .await?;
    let role_rows: Vec<(Uuid, Uuid, String, String)> = sqlx::query_as(
        "SELECT ur.user_id, r.id, r.code, r.name FROM user_roles ur
         JOIN roles r ON r.id = ur.role_id
         WHERE ur.user_id = ANY($1) ORDER BY r.name",
    )
    .bind(&ids)
    .fetch_all(&state.db)
    .await?;

    let mut depts: HashMap<Uuid, Vec<Ref>> = HashMap::new();
    for (uid, id, name) in dept_rows {
        depts.entry(uid).or_default().push(Ref { id, name });
    }
    let mut roles: HashMap<Uuid, Vec<RoleRef>> = HashMap::new();
    for (uid, id, code, name) in role_rows {
        roles.entry(uid).or_default().push(RoleRef { id, code, name });
    }

    Ok(rows
        .into_iter()
        .map(|r| User {
            full_name: format!("{} {}", r.first_name, r.last_name),
            departments: depts.remove(&r.id).unwrap_or_default(),
            roles: roles.remove(&r.id).unwrap_or_default(),
            id: r.id,
            email: r.email,
            first_name: r.first_name,
            last_name: r.last_name,
            middle_name: r.middle_name,
            position: r.position,
            phone: r.phone,
            mobile: r.mobile,
            birthday: r.birthday,
            gender: r.gender,
            about: r.about,
            avatar_file_id: r.avatar_file_id,
            timezone: r.timezone,
            locale: r.locale,
            is_active: r.is_active,
            invite_pending: r.invite_pending,
            is_guest: r.is_guest,
            invited_at: r.invited_at,
            two_factor_enabled: r.two_factor_enabled,
            last_login_at: r.last_login_at,
            created_at: r.created_at,
            online: r.last_seen_at.is_some_and(|t| Utc::now() - t < chrono::Duration::minutes(5)),
            last_seen_at: r.last_seen_at,
        })
        .collect())
}

pub async fn fetch_user(state: &AppState, id: Uuid) -> AppResult<User> {
    let row: UserRow = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {USER_COLUMNS} FROM users u WHERE u.id = $1")))
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound("user"))?;
    Ok(hydrate(state, vec![row]).await?.remove(0))
}

// ---------- list ----------

#[derive(Deserialize)]
pub struct ListQuery {
    q: Option<String>,
    department_id: Option<Uuid>,
    role_id: Option<Uuid>,
    /// active (default) | inactive | all — anything but "active" needs users.manage.
    status: Option<String>,
    /// Pickers include guests (the directory does not).
    #[serde(default)]
    with_guests: bool,
    #[serde(flatten)]
    page: PageParams,
}

async fn list_users(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(q): Query<ListQuery>,
) -> AppResult<Json<Page<User>>> {
    auth.require(perm::USERS_VIEW)?;
    let status = q.status.as_deref().unwrap_or("active");
    if status != "active" {
        auth.require(perm::USERS_MANAGE)?;
    }

    let build = |select: &str| {
        let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(select);
        qb.push(" FROM users u WHERE NOT u.is_system AND NOT u.join_pending");
        if !q.with_guests {
            qb.push(" AND NOT u.is_guest");
        }
        match status {
            "inactive" => {
                qb.push(" AND u.is_active = FALSE");
            }
            "all" => {}
            _ => {
                qb.push(" AND u.is_active = TRUE");
            }
        }
        if let Some(term) = clean(q.q.clone()) {
            let like = format!("%{}%", term.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
            qb.push(" AND (u.first_name ILIKE ")
                .push_bind(like.clone())
                .push(" OR u.last_name ILIKE ")
                .push_bind(like.clone())
                .push(" OR u.email ILIKE ")
                .push_bind(like.clone())
                .push(" OR u.position ILIKE ")
                .push_bind(like.clone())
                .push(" OR (u.first_name || ' ' || u.last_name) ILIKE ")
                .push_bind(like)
                .push(")");
        }
        if let Some(d) = q.department_id {
            qb.push(" AND EXISTS (SELECT 1 FROM user_departments ud WHERE ud.user_id = u.id AND ud.department_id = ")
                .push_bind(d)
                .push(")");
        }
        if let Some(r) = q.role_id {
            qb.push(" AND EXISTS (SELECT 1 FROM user_roles ur WHERE ur.user_id = u.id AND ur.role_id = ")
                .push_bind(r)
                .push(")");
        }
        qb
    };

    let total: i64 = build("SELECT COUNT(*)").build_query_scalar().fetch_one(&state.db).await?;
    let mut qb = build(&format!("SELECT {USER_COLUMNS}"));
    qb.push(" ORDER BY u.last_name, u.first_name, u.id LIMIT ")
        .push_bind(q.page.limit())
        .push(" OFFSET ")
        .push_bind(q.page.offset());
    let rows: Vec<UserRow> = qb.build_query_as().fetch_all(&state.db).await?;

    Ok(Json(Page {
        items: hydrate(&state, rows).await?,
        total,
        limit: q.page.limit(),
        offset: q.page.offset(),
    }))
}

async fn get_user(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<User>> {
    if auth.id != id {
        auth.require(perm::USERS_VIEW)?;
    }
    let user = fetch_user(&state, id).await?;
    if !user.is_active && auth.id != id {
        auth.require(perm::USERS_MANAGE)?;
    }
    Ok(Json(user))
}

// ---------- shared field validation ----------

#[derive(Deserialize, Default)]
pub struct ProfileFields {
    first_name: Option<String>,
    last_name: Option<String>,
    #[serde(default, deserialize_with = "double_option")]
    middle_name: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    phone: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    mobile: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    birthday: Option<Option<NaiveDate>>,
    #[serde(default, deserialize_with = "double_option")]
    gender: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    about: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    avatar_file_id: Option<Option<Uuid>>,
    timezone: Option<String>,
    locale: Option<String>,
}

fn validate_gender(g: &Option<String>) -> AppResult<()> {
    match g.as_deref() {
        None | Some("male") | Some("female") | Some("other") => Ok(()),
        _ => Err(AppError::bad("gender must be male, female or other")),
    }
}

fn opt_text(v: Option<String>, field: &str, max: usize) -> AppResult<Option<String>> {
    let v = clean(v);
    if v.as_ref().is_some_and(|s| s.chars().count() > max) {
        return Err(AppError::bad(format!("{field} is too long (max {max})")));
    }
    Ok(v)
}

/// Appends `SET` clauses for the profile fields present in a PATCH body.
async fn push_profile_fields(
    state: &AppState,
    qb: &mut QueryBuilder<Postgres>,
    f: ProfileFields,
    allowed_owners: &[Uuid],
) -> AppResult<()> {
    if let Some(v) = f.first_name {
        qb.push(", first_name = ").push_bind(required("first name", &v, 100)?);
    }
    if let Some(v) = f.last_name {
        qb.push(", last_name = ").push_bind(required("last name", &v, 100)?);
    }
    if let Some(v) = f.middle_name {
        qb.push(", middle_name = ").push_bind(opt_text(v, "middle name", 100)?);
    }
    if let Some(v) = f.phone {
        qb.push(", phone = ").push_bind(opt_text(v, "phone", 50)?);
    }
    if let Some(v) = f.mobile {
        qb.push(", mobile = ").push_bind(opt_text(v, "mobile", 50)?);
    }
    if let Some(v) = f.birthday {
        qb.push(", birthday = ").push_bind(v);
    }
    if let Some(v) = f.gender {
        let v = clean(v);
        validate_gender(&v)?;
        qb.push(", gender = ").push_bind(v);
    }
    if let Some(v) = f.about {
        qb.push(", about = ").push_bind(opt_text(v, "about", 4000)?);
    }
    if let Some(v) = f.avatar_file_id {
        if let Some(file_id) = v {
            // Avatars must be public images the caller uploaded.
            let ok: Option<bool> = sqlx::query_scalar(
                "SELECT is_public AND mime_type LIKE 'image/%' AND owner_id = ANY($2) FROM files WHERE id = $1",
            )
            .bind(file_id)
            .bind(allowed_owners)
            .fetch_optional(&state.db)
            .await?;
            if ok != Some(true) {
                return Err(AppError::bad("avatar must be a public image you uploaded"));
            }
        }
        qb.push(", avatar_file_id = ").push_bind(v);
    }
    if let Some(v) = f.timezone {
        qb.push(", timezone = ").push_bind(required("timezone", &v, 64)?);
    }
    if let Some(v) = f.locale {
        qb.push(", locale = ").push_bind(required("locale", &v, 16)?);
    }
    Ok(())
}

// ---------- create ----------

#[derive(Deserialize)]
pub struct CreateUser {
    email: String,
    /// Set a password directly, or leave empty and set `send_invite` to email an invitation.
    password: Option<String>,
    #[serde(default)]
    send_invite: bool,
    first_name: String,
    last_name: String,
    middle_name: Option<String>,
    position: Option<String>,
    phone: Option<String>,
    mobile: Option<String>,
    #[serde(default)]
    department_ids: Vec<Uuid>,
    #[serde(default)]
    role_ids: Vec<Uuid>,
}

async fn create_user(
    State(state): State<AppState>,
    auth: AuthUser,
    ClientIp(ip): ClientIp,
    Json(body): Json<CreateUser>,
) -> AppResult<Json<User>> {
    // Inviting by email is enough for those who may only invite.
    if !(auth.can(perm::USERS_INVITE) && body.send_invite && body.role_ids.is_empty()) {
        auth.require(perm::USERS_MANAGE)?;
    }
    if !body.role_ids.is_empty() {
        auth.require(perm::ROLES_MANAGE)?;
    }
    let email = normalize_email(&body.email)?;
    let first_name = required("first name", &body.first_name, 100)?;
    let last_name = required("last name", &body.last_name, 100)?;
    let invite = body.send_invite;
    let hash = match (clean(body.password), invite) {
        (Some(p), false) => {
            password::validate_password(&p)?;
            password::hash_password(p).await?
        }
        // Not a valid argon2 hash, so no password can match until the invitation is accepted.
        (None, true) => "!invited".to_string(),
        (Some(_), true) => return Err(AppError::bad("either set a password or send an invitation, not both")),
        (None, false) => return Err(AppError::bad("set a password or send an invitation")),
    };

    let mut tx = state.db.begin().await?;
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO users (email, password_hash, first_name, last_name, middle_name, position, phone, mobile, invite_pending)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) RETURNING id",
    )
    .bind(&email)
    .bind(hash)
    .bind(&first_name)
    .bind(last_name)
    .bind(opt_text(body.middle_name, "middle name", 100)?)
    .bind(opt_text(body.position, "position", 200)?)
    .bind(opt_text(body.phone, "phone", 50)?)
    .bind(opt_text(body.mobile, "mobile", 50)?)
    .bind(invite)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| {
        if crate::error::is_unique_violation(&e) {
            AppError::Conflict("an employee with this email already exists".into())
        } else {
            e.into()
        }
    })?;

    sqlx::query(
        "INSERT INTO user_roles (user_id, role_id)
         SELECT $1, id FROM roles WHERE code = 'employee' OR id = ANY($2)",
    )
    .bind(id)
    .bind(&body.role_ids)
    .execute(&mut *tx)
    .await?;

    set_departments(&mut tx, id, &body.department_ids).await?;
    audit::record(&mut *tx, Some(auth.id), if invite { "user.invite" } else { "user.create" }, "user", Some(id.to_string()), json!({ "email": email }), ip.as_deref()).await;
    tx.commit().await?;
    // The general chat and department chats follow the company.
    let _ = crate::im::sync_audiences(&state, Some(id)).await;

    if invite {
        send_invitation(&state, id, auth.id).await?;
    }
    notify(
        &state,
        &[id],
        NewNotification {
            kind: "user.welcome",
            title: format!("Welcome, {first_name}!"),
            body: Some("Complete your profile so colleagues can find and recognise you.".into()),
            link: Some("/profile".into()),
            actor: Some(auth.id),
        },
    )
    .await;
    notify_departments_joined(&state, id, &[], &body.department_ids, auth.id).await;
    Ok(Json(fetch_user(&state, id).await?))
}

/// Tells a user about departments they were newly added to.
async fn notify_departments_joined(state: &AppState, user: Uuid, before: &[Uuid], after: &[Uuid], actor: Uuid) {
    let added: Vec<Uuid> = after.iter().copied().filter(|d| !before.contains(d)).collect();
    if added.is_empty() {
        return;
    }
    let names: Vec<(Uuid, String)> = sqlx::query_as("SELECT id, name FROM departments WHERE id = ANY($1)")
        .bind(&added)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();
    for (dept, name) in names {
        notify(
            state,
            &[user],
            NewNotification {
                kind: "department.added",
                title: format!("You were added to {name}"),
                body: None,
                link: Some(format!("/structure?d={dept}")),
                actor: Some(actor),
            },
        )
        .await;
    }
}

async fn set_departments(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    user_id: Uuid,
    department_ids: &[Uuid],
) -> AppResult<()> {
    sqlx::query("DELETE FROM user_departments WHERE user_id = $1")
        .bind(user_id)
        .execute(&mut **tx)
        .await?;
    if department_ids.is_empty() {
        return Ok(());
    }
    let inserted = sqlx::query(
        "INSERT INTO user_departments (user_id, department_id)
         SELECT $1, id FROM departments WHERE id = ANY($2)",
    )
    .bind(user_id)
    .bind(department_ids)
    .execute(&mut **tx)
    .await?
    .rows_affected();
    let mut unique = department_ids.to_vec();
    unique.sort();
    unique.dedup();
    if inserted as usize != unique.len() {
        return Err(AppError::bad("one or more departments do not exist"));
    }
    Ok(())
}

// ---------- update (admin) ----------

#[derive(Deserialize)]
pub struct UpdateUser {
    email: Option<String>,
    #[serde(default, deserialize_with = "double_option")]
    position: Option<Option<String>>,
    is_active: Option<bool>,
    department_ids: Option<Vec<Uuid>>,
    #[serde(flatten)]
    profile: ProfileFields,
}

async fn update_user(
    State(state): State<AppState>,
    auth: AuthUser,
    ClientIp(ip): ClientIp,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateUser>,
) -> AppResult<Json<User>> {
    auth.require(perm::USERS_MANAGE)?;
    let current = fetch_user(&state, id).await?;

    if body.is_active == Some(false) && current.is_active {
        if id == auth.id {
            return Err(AppError::bad("you cannot deactivate your own account"));
        }
        ensure_not_last_admin(&state, id).await?;
    }

    let mut tx = state.db.begin().await?;
    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new("UPDATE users SET updated_at = now()");
    if let Some(email) = &body.email {
        qb.push(", email = ").push_bind(normalize_email(email)?);
    }
    if let Some(v) = body.position {
        qb.push(", position = ").push_bind(opt_text(v, "position", 200)?);
    }
    if let Some(v) = body.is_active {
        qb.push(", is_active = ").push_bind(v);
    }
    push_profile_fields(&state, &mut qb, body.profile, &[id, auth.id]).await?;
    qb.push(" WHERE id = ").push_bind(id);
    qb.build().execute(&mut *tx).await.map_err(|e| {
        if crate::error::is_unique_violation(&e) {
            AppError::Conflict("an employee with this email already exists".into())
        } else {
            e.into()
        }
    })?;

    if let Some(depts) = &body.department_ids {
        set_departments(&mut tx, id, depts).await?;
    }

    let action = match body.is_active {
        Some(false) if current.is_active => "user.deactivate",
        Some(true) if !current.is_active => "user.activate",
        _ => "user.update",
    };
    audit::record(&mut *tx, Some(auth.id), action, "user", Some(id.to_string()), json!({}), ip.as_deref()).await;
    tx.commit().await?;
    // The general chat and department chats follow the company.
    let _ = crate::im::sync_audiences(&state, Some(id)).await;

    if body.is_active.is_some() {
        invalidate_authz(&state, &[id]).await?;
        if body.is_active == Some(false) {
            tokens::revoke_all_sessions(&state, id, None).await?;
            tokens::revoke_link_tokens(&state, super::auth::INVITE_SCOPE, id).await?;
        }
    }
    if let Some(depts) = &body.department_ids {
        let before: Vec<Uuid> = current.departments.iter().map(|d| d.id).collect();
        notify_departments_joined(&state, id, &before, depts, auth.id).await;
    }
    Ok(Json(fetch_user(&state, id).await?))
}

/// Refuses an operation that would leave the company without an active administrator.
pub async fn ensure_not_last_admin(state: &AppState, losing_user: Uuid) -> AppResult<()> {
    let others: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT u.id) FROM users u
         JOIN user_roles ur ON ur.user_id = u.id
         JOIN role_permissions rp ON rp.role_id = ur.role_id AND rp.permission = '*'
         WHERE u.is_active AND u.id <> $1",
    )
    .bind(losing_user)
    .fetch_one(&state.db)
    .await?;
    if others == 0 {
        return Err(AppError::bad("NERP needs at least one active administrator"));
    }
    Ok(())
}

// ---------- roles ----------

#[derive(Deserialize)]
pub struct SetRoles {
    role_ids: Vec<Uuid>,
}

async fn set_roles(
    State(state): State<AppState>,
    auth: AuthUser,
    ClientIp(ip): ClientIp,
    Path(id): Path<Uuid>,
    Json(body): Json<SetRoles>,
) -> AppResult<Json<User>> {
    auth.require(perm::ROLES_MANAGE)?;
    let user = fetch_user(&state, id).await?;

    let mut tx = state.db.begin().await?;
    sqlx::query("DELETE FROM user_roles WHERE user_id = $1").bind(id).execute(&mut *tx).await?;
    // Every user keeps the default employee role.
    sqlx::query(
        "INSERT INTO user_roles (user_id, role_id)
         SELECT $1, id FROM roles WHERE code = 'employee' OR id = ANY($2)",
    )
    .bind(id)
    .bind(&body.role_ids)
    .execute(&mut *tx)
    .await?;

    let admins_left: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT u.id) FROM users u
         JOIN user_roles ur ON ur.user_id = u.id
         JOIN role_permissions rp ON rp.role_id = ur.role_id AND rp.permission = '*'
         WHERE u.is_active",
    )
    .fetch_one(&mut *tx)
    .await?;
    if admins_left == 0 {
        return Err(AppError::bad("NERP needs at least one active administrator"));
    }

    audit::record(
        &mut *tx,
        Some(auth.id),
        "user.roles",
        "user",
        Some(id.to_string()),
        json!({ "before": user.roles.iter().map(|r| &r.code).collect::<Vec<_>>(), "role_ids": body.role_ids }),
        ip.as_deref(),
    )
    .await;
    tx.commit().await?;

    invalidate_authz(&state, &[id]).await?;
    let updated = fetch_user(&state, id).await?;
    let before: Vec<Uuid> = user.roles.iter().map(|r| r.id).collect();
    let added: Vec<&str> = updated.roles.iter().filter(|r| !before.contains(&r.id)).map(|r| r.name.as_str()).collect();
    if !added.is_empty() {
        notify(
            &state,
            &[id],
            NewNotification {
                kind: "user.roles",
                title: format!("You were given the {} role{}", added.join(", "), if added.len() > 1 { "s" } else { "" }),
                body: Some("You may see new sections in the menu.".into()),
                link: Some("/profile".into()),
                actor: Some(auth.id),
            },
        )
        .await;
    }
    // Let the user's open tabs reload their permissions.
    crate::realtime::publish(&state, Some(vec![id]), "me.changed", json!({})).await;
    Ok(Json(updated))
}

// ---------- admin password reset ----------

#[derive(Deserialize)]
pub struct ResetPassword {
    password: String,
}

async fn reset_password(
    State(state): State<AppState>,
    auth: AuthUser,
    ClientIp(ip): ClientIp,
    Path(id): Path<Uuid>,
    Json(body): Json<ResetPassword>,
) -> AppResult<Json<serde_json::Value>> {
    auth.require(perm::USERS_MANAGE)?;
    password::validate_password(&body.password)?;
    let hash = password::hash_password(body.password).await?;
    let updated = sqlx::query(
        "UPDATE users SET password_hash = $1, password_changed_at = now(), updated_at = now() WHERE id = $2",
    )
    .bind(hash)
    .bind(id)
    .execute(&state.db)
    .await?
    .rows_affected();
    if updated == 0 {
        return Err(AppError::NotFound("user"));
    }
    tokens::revoke_all_sessions(&state, id, None).await?;
    invalidate_authz(&state, &[id]).await?;
    audit::record(&state.db, Some(auth.id), "user.password_reset", "user", Some(id.to_string()), json!({}), ip.as_deref()).await;
    notify(
        &state,
        &[id],
        NewNotification {
            kind: "security.password_reset",
            title: "An administrator reset your password".into(),
            body: Some("You were signed out on all devices.".into()),
            link: Some("/profile#security".into()),
            actor: Some(auth.id),
        },
    )
    .await;
    Ok(Json(json!({ "ok": true })))
}

// ---------- invitations & account security (admin) ----------

async fn resend_invite(
    State(state): State<AppState>,
    auth: AuthUser,
    ClientIp(ip): ClientIp,
    Path(id): Path<Uuid>,
) -> AppResult<Json<serde_json::Value>> {
    if !auth.can(perm::USERS_INVITE) {
        auth.require(perm::USERS_MANAGE)?;
    }
    let user = fetch_user(&state, id).await?;
    if !user.is_active {
        return Err(AppError::bad("reactivate this employee before inviting them"));
    }
    send_invitation(&state, id, auth.id).await?;
    audit::record(&state.db, Some(auth.id), "user.invite", "user", Some(id.to_string()), json!({ "email": user.email, "resend": true }), ip.as_deref()).await;
    Ok(Json(json!({ "ok": true })))
}

async fn reset_two_factor(
    State(state): State<AppState>,
    auth: AuthUser,
    ClientIp(ip): ClientIp,
    Path(id): Path<Uuid>,
) -> AppResult<Json<serde_json::Value>> {
    auth.require(perm::USERS_MANAGE)?;
    fetch_user(&state, id).await?;
    disable_two_factor(&state, id).await?;
    audit::record(&state.db, Some(auth.id), "user.2fa_reset", "user", Some(id.to_string()), json!({}), ip.as_deref()).await;
    notify(
        &state,
        &[id],
        NewNotification {
            kind: "security.2fa_reset",
            title: "Two-factor authentication was turned off by an administrator".into(),
            body: Some("Turn it back on from your profile to keep your account protected.".into()),
            link: Some("/profile#security".into()),
            actor: Some(auth.id),
        },
    )
    .await;
    Ok(Json(json!({ "ok": true })))
}

async fn user_sessions(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Vec<SessionInfo>>> {
    auth.require(perm::USERS_MANAGE)?;
    Ok(Json(sessions_for(&state, id, None).await?))
}

async fn revoke_user_sessions(
    State(state): State<AppState>,
    auth: AuthUser,
    ClientIp(ip): ClientIp,
    Path(id): Path<Uuid>,
) -> AppResult<Json<serde_json::Value>> {
    auth.require(perm::USERS_MANAGE)?;
    if id == auth.id {
        return Err(AppError::bad("use your profile to manage your own sessions"));
    }
    tokens::revoke_all_sessions(&state, id, None).await?;
    audit::record(&state.db, Some(auth.id), "user.sessions_revoked", "user", Some(id.to_string()), json!({}), ip.as_deref()).await;
    Ok(Json(json!({ "ok": true })))
}

// ---------- self-service profile ----------

async fn update_profile(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(body): Json<ProfileFields>,
) -> AppResult<Json<User>> {
    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new("UPDATE users SET updated_at = now()");
    push_profile_fields(&state, &mut qb, body, &[auth.id]).await?;
    qb.push(" WHERE id = ").push_bind(auth.id);
    qb.build().execute(&state.db).await?;
    Ok(Json(fetch_user(&state, auth.id).await?))
}

pub(crate) fn csv_cell(s: &str) -> String {
    // Cells a spreadsheet would run as a formula are made plain text.
    let s = if s.starts_with(['=', '+', '-', '@', '\t', '\r']) { format!("'{s}") } else { s.to_string() };
    if s.contains([',', '"', '\n', '\r']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s }
}

/// The employee list as CSV (opens in Excel), for those who may see employees.
async fn export_users(State(state): State<AppState>, auth: AuthUser) -> AppResult<axum::response::Response> {
    auth.require(perm::USERS_VIEW)?;
    let rows: Vec<UserRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {USER_COLUMNS} FROM users u WHERE u.is_active ORDER BY u.last_name, u.first_name"))).fetch_all(&state.db).await?;
    let users = hydrate(&state, rows).await?;
    let mut out = String::from("\u{feff}First name,Last name,Email,Position,Departments,Phone,Mobile,Birthday\r\n");
    for u in users {
        let cells = [
            u.first_name, u.last_name, u.email, u.position.unwrap_or_default(), u.departments.iter().map(|d| d.name.clone()).collect::<Vec<_>>().join("; "),
            u.phone.unwrap_or_default(), u.mobile.unwrap_or_default(), u.birthday.map(|b| b.to_string()).unwrap_or_default(),
        ];
        out.push_str(&cells.iter().map(|c| csv_cell(c)).collect::<Vec<_>>().join(","));
        out.push_str("\r\n");
    }
    let mut res = axum::response::IntoResponse::into_response(out);
    res.headers_mut().insert(axum::http::header::CONTENT_TYPE, axum::http::HeaderValue::from_static("text/csv; charset=utf-8"));
    res.headers_mut().insert(axum::http::header::CONTENT_DISPOSITION, axum::http::HeaderValue::from_static("attachment; filename=\"employees.csv\""));
    Ok(res)
}
