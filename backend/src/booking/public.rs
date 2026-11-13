//! Self-booking forms (made by "Manage booking") and the public pages clients use: the form
//! at `/book/<slug>`, and the link each client gets to see, confirm or cancel a booking.

use axum::{
    extract::{Path, Query, State},
    routing::{get, patch},
    Json, Router,
};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use redis::AsyncCommands;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use super::{
    bookings::{freed, hash, load, make, NewBooking},
    free_slots, load_services, manage, service_json, Resource, RESOURCE_SELECT,
};
use crate::{
    auth::{AuthUser, ClientIp},
    error::{AppError, AppResult},
    routes::{clean, required, slugify},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/booking/forms", get(list).post(create))
        .route("/booking/forms/{id}", patch(update).delete(remove))
        .route("/public/booking/manage/{token}", get(manage_view).post(manage_act))
        .route("/public/booking/{slug}", get(form).post(book))
        .route("/public/booking/{slug}/slots", get(slots))
}

#[derive(sqlx::FromRow)]
struct Form {
    id: Uuid,
    name: String,
    slug: String,
    intro: Option<String>,
    service_ids: Vec<Uuid>,
    resource_ids: Vec<Uuid>,
    ask_phone: bool,
    phone_required: bool,
    choose_resource: bool,
    waitlist: bool,
    thanks: Option<String>,
    is_active: bool,
}

const FORM_SELECT: &str = "SELECT id, name, slug, intro, service_ids, resource_ids, ask_phone, phone_required, choose_resource, waitlist, thanks, is_active FROM booking_forms";

fn form_json(f: &Form) -> Value {
    json!({ "id": f.id, "name": f.name, "slug": f.slug, "intro": f.intro, "service_ids": f.service_ids, "resource_ids": f.resource_ids,
            "ask_phone": f.ask_phone, "phone_required": f.phone_required, "choose_resource": f.choose_resource, "waitlist": f.waitlist,
            "thanks": f.thanks, "is_active": f.is_active })
}

async fn list(State(state): State<AppState>, _auth: AuthUser) -> AppResult<Json<Value>> {
    let rows: Vec<Form> = sqlx::query_as(sqlx::AssertSqlSafe(format!("{FORM_SELECT} ORDER BY name"))).fetch_all(&state.db).await?;
    Ok(Json(json!({ "items": rows.iter().map(form_json).collect::<Vec<_>>() })))
}

#[derive(Deserialize)]
struct FormInput {
    name: Option<String>,
    slug: Option<String>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    intro: Option<Option<String>>,
    service_ids: Option<Vec<Uuid>>,
    resource_ids: Option<Vec<Uuid>>,
    ask_phone: Option<bool>,
    phone_required: Option<bool>,
    choose_resource: Option<bool>,
    waitlist: Option<bool>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    thanks: Option<Option<String>>,
    is_active: Option<bool>,
}

async fn create(State(state): State<AppState>, auth: AuthUser, Json(b): Json<FormInput>) -> AppResult<Json<Value>> {
    manage(&auth)?;
    let name = required("name", b.name.as_deref().unwrap_or(""), 200)?;
    let base = slugify(b.slug.as_deref().unwrap_or(&name), "book");
    let mut slug = base.clone();
    for n in 2.. {
        let taken: bool = sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM booking_forms WHERE slug = $1) AS "v!""#, &slug).fetch_one(&state.db).await?;
        if !taken {
            break;
        }
        slug = format!("{base}-{n}");
    }
    sqlx::query!("INSERT INTO booking_forms (name, slug, intro, service_ids, resource_ids, ask_phone, phone_required, choose_resource, waitlist, thanks, created_by)
         VALUES ($1, $2, $3, $4, $5, COALESCE($6, TRUE), COALESCE($7, FALSE), COALESCE($8, TRUE), COALESCE($9, TRUE), $10, $11)", &name, &slug, clean(b.intro.flatten()), &b.service_ids.unwrap_or_default(), &b.resource_ids.unwrap_or_default(), b.ask_phone, b.phone_required, b.choose_resource, b.waitlist, clean(b.thanks.flatten()), auth.id)
    .execute(&state.db)
    .await?;
    list(State(state), auth).await
}

async fn update(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<FormInput>) -> AppResult<Json<Value>> {
    manage(&auth)?;
    let name = match &b.name {
        Some(n) => Some(required("name", n, 200)?),
        None => None,
    };
    let n = sqlx::query!("UPDATE booking_forms SET name = COALESCE($2, name), intro = CASE WHEN $3 THEN $4 ELSE intro END, service_ids = COALESCE($5, service_ids),
            resource_ids = COALESCE($6, resource_ids), ask_phone = COALESCE($7, ask_phone), phone_required = COALESCE($8, phone_required),
            choose_resource = COALESCE($9, choose_resource), waitlist = COALESCE($10, waitlist), thanks = CASE WHEN $11 THEN $12 ELSE thanks END,
            is_active = COALESCE($13, is_active) WHERE id = $1", id, name, b.intro.is_some(), clean(b.intro.flatten()), b.service_ids.as_deref(), b.resource_ids.as_deref(), b.ask_phone, b.phone_required, b.choose_resource, b.waitlist, b.thanks.is_some(), clean(b.thanks.flatten()), b.is_active)
    .execute(&state.db)
    .await?
    .rows_affected();
    if n == 0 {
        return Err(AppError::NotFound("form"));
    }
    list(State(state), auth).await
}

async fn remove(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    manage(&auth)?;
    sqlx::query!("DELETE FROM booking_forms WHERE id = $1", id).execute(&state.db).await?;
    list(State(state), auth).await
}

// ---------------------------------------------------------------------------
// The public form
// ---------------------------------------------------------------------------

async fn active_form(state: &AppState, slug: &str) -> AppResult<Form> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{FORM_SELECT} WHERE slug = $1 AND is_active")))
        .bind(slug)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound("form"))
}

/// The resources a form offers for a service.
async fn eligible(state: &AppState, f: &Form, service: Option<Uuid>) -> AppResult<Vec<Resource>> {
    let svc_res: Vec<Uuid> = match service {
        Some(s) => sqlx::query_scalar!("SELECT resource_ids FROM booking_services WHERE id = $1", s).fetch_optional(&state.db).await?.unwrap_or_default(),
        None => vec![],
    };
    let all: Vec<Resource> = sqlx::query_as(sqlx::AssertSqlSafe(format!("{RESOURCE_SELECT} WHERE r.is_active ORDER BY r.sort, r.name"))).fetch_all(&state.db).await?;
    Ok(all
        .into_iter()
        .filter(|r| (f.resource_ids.is_empty() || f.resource_ids.contains(&r.id)) && (svc_res.is_empty() || svc_res.contains(&r.id)))
        .collect())
}

async fn form(State(state): State<AppState>, Path(slug): Path<String>) -> AppResult<Json<Value>> {
    let f = active_form(&state, &slug).await?;
    let services: Vec<Value> = load_services(&state)
        .await?
        .iter()
        .filter(|s| s.is_active && (f.service_ids.is_empty() || f.service_ids.contains(&s.id) || s.is_extra))
        .map(service_json)
        .collect();
    let resources = eligible(&state, &f, None).await?;
    let currency: String = sqlx::query_scalar!("SELECT currency FROM booking_settings WHERE id").fetch_one(&state.db).await?;
    Ok(Json(json!({
        "name": f.name, "intro": f.intro, "ask_phone": f.ask_phone, "phone_required": f.phone_required, "choose_resource": f.choose_resource, "waitlist": f.waitlist,
        "currency": currency,
        "services": services,
        "resources": resources.iter().map(|r| json!({ "id": r.id, "name": r.name, "kind": r.kind, "description": r.description, "timezone": r.timezone })).collect::<Vec<_>>(),
    })))
}

#[derive(Deserialize)]
struct SlotsQuery {
    service_id: Uuid,
    date: NaiveDate,
    resource_id: Option<Uuid>,
}

/// Free times on a day: each with the resources free then.
async fn slots(State(state): State<AppState>, Path(slug): Path<String>, Query(q): Query<SlotsQuery>) -> AppResult<Json<Value>> {
    let f = active_form(&state, &slug).await?;
    let service = load_services(&state).await?.into_iter().find(|s| s.id == q.service_id && s.is_active && !s.is_extra).ok_or(AppError::NotFound("service"))?;
    let mut by: std::collections::BTreeMap<DateTime<Utc>, Vec<Uuid>> = Default::default();
    for r in eligible(&state, &f, Some(service.id)).await? {
        if q.resource_id.is_some_and(|x| x != r.id) {
            continue;
        }
        for s in free_slots(&state, &r, Duration::minutes(service.duration_minutes as i64), q.date, true).await? {
            by.entry(s).or_default().push(r.id);
        }
    }
    Ok(Json(json!({ "slots": by.into_iter().map(|(t, r)| json!({ "start": t, "resource_ids": r })).collect::<Vec<_>>() })))
}

#[derive(Deserialize)]
struct BookInput {
    service_id: Uuid,
    #[serde(default)]
    extra_ids: Vec<Uuid>,
    starts_at: DateTime<Utc>,
    resource_id: Option<Uuid>,
    name: String,
    email: String,
    phone: Option<String>,
    note: Option<String>,
    /// Join the waiting list for this time.
    #[serde(default)]
    waitlist: bool,
    /// Left empty by people; filled by bots.
    #[serde(default)]
    website: String,
}

async fn limited(state: &AppState, key: String, max: i64, secs: i64) -> AppResult<bool> {
    let mut redis = state.redis.clone();
    let n: i64 = redis.incr(&key, 1).await?;
    if n == 1 {
        let _: () = redis.expire(&key, secs).await?;
    }
    Ok(n > max)
}

async fn book(State(state): State<AppState>, ClientIp(ip): ClientIp, Path(slug): Path<String>, Json(b): Json<BookInput>) -> AppResult<Json<Value>> {
    let f = active_form(&state, &slug).await?;
    if !b.website.is_empty() {
        return Err(AppError::bad("something went wrong; please try again"));
    }
    let email = b.email.trim().to_lowercase();
    if !email.contains('@') {
        return Err(AppError::bad("please enter your email"));
    }
    let phone = clean(b.phone.clone());
    if f.phone_required && phone.is_none() {
        return Err(AppError::bad("please enter your phone number"));
    }
    if b.waitlist && !f.waitlist {
        return Err(AppError::bad("this form has no waiting list"));
    }
    let service = load_services(&state).await?.into_iter().find(|s| s.id == b.service_id && s.is_active && !s.is_extra && (f.service_ids.is_empty() || f.service_ids.contains(&s.id))).ok_or_else(|| AppError::bad("choose a service"))?;
    let resources = eligible(&state, &f, Some(service.id)).await?;
    let duration = Duration::minutes(service.duration_minutes as i64);
    // The resource: as chosen (if the form lets clients choose), else the first free one.
    let mut pick: Option<Uuid> = None;
    for r in &resources {
        if b.resource_id.is_some_and(|x| x != r.id) || (b.resource_id.is_some() && !f.choose_resource) {
            continue;
        }
        if b.waitlist {
            pick = Some(r.id);
            break;
        }
        let day = b.starts_at.with_timezone(&r.timezone.parse::<chrono_tz::Tz>().unwrap_or(chrono_tz::UTC)).date_naive();
        if free_slots(&state, r, duration, day, true).await?.contains(&b.starts_at) {
            pick = Some(r.id);
            break;
        }
    }
    let Some(resource) = pick else {
        return Err(AppError::Conflict(if f.waitlist { "that time was just taken; choose another, or join the waiting list".into() } else { "that time was just taken; please choose another".into() }));
    };
    let who = ip.unwrap_or_else(|| "unknown".into());
    if limited(&state, format!("booking:ip:{who}"), 20, 600).await? || limited(&state, format!("booking:email:{email}"), 5, 600).await? {
        return Err(AppError::TooManyRequests("too many bookings in a short time; please try again later".into()));
    }
    let made = make(
        &state,
        None,
        NewBooking {
            resource_ids: vec![resource],
            service_id: Some(service.id),
            extra_ids: b.extra_ids,
            starts_at: b.starts_at,
            ends_at: None,
            client_name: b.name,
            client_email: Some(email),
            client_phone: phone,
            contact_id: None,
            deal_id: None,
            note: b.note,
            overbook: false,
            waitlist: b.waitlist,
        },
        "form",
        Some(f.id),
    )
    .await?;
    Ok(Json(json!({
        "number": made.booking.number,
        "status": made.booking.status,
        "starts_at": made.booking.starts_at,
        "manage_url": format!("{}/book/manage/{}", state.config.public_url, made.token),
        "thanks": f.thanks,
    })))
}

// ---------------------------------------------------------------------------
// The client's link
// ---------------------------------------------------------------------------

async fn by_token(state: &AppState, token: &str) -> AppResult<Uuid> {
    sqlx::query_scalar!("SELECT id FROM bookings WHERE token_hash = $1", hash(token)).fetch_optional(&state.db).await?.ok_or(AppError::NotFound("booking"))
}

async fn client_view(state: &AppState, id: Uuid) -> AppResult<Value> {
    let b = load(state, id).await?;
    let names: Vec<String> = sqlx::query_scalar!("SELECT r.name FROM booking_resources_used u JOIN booking_resources r ON r.id = u.resource_id WHERE u.booking_id = $1", id).fetch_all(&state.db).await?;
    let tz: Option<String> = sqlx::query_scalar!("SELECT r.timezone FROM booking_resources_used u JOIN booking_resources r ON r.id = u.resource_id WHERE u.booking_id = $1 LIMIT 1", id).fetch_optional(&state.db).await?;
    let can_change = ["booked", "confirmed", "waitlist"].contains(&b.status.as_str()) && b.starts_at > Utc::now();
    Ok(json!({
        "number": b.number, "status": b.status, "starts_at": b.starts_at, "ends_at": b.ends_at, "timezone": tz.unwrap_or_else(|| "UTC".into()),
        "service": b.service_name, "with": names, "client_name": b.client_name, "price": b.price,
        "can_change": can_change,
    }))
}

async fn manage_view(State(state): State<AppState>, Path(token): Path<String>) -> AppResult<Json<Value>> {
    let id = by_token(&state, &token).await?;
    Ok(Json(client_view(&state, id).await?))
}

#[derive(Deserialize)]
struct Act {
    /// confirm | cancel
    action: String,
}

async fn manage_act(State(state): State<AppState>, Path(token): Path<String>, Json(a): Json<Act>) -> AppResult<Json<Value>> {
    let id = by_token(&state, &token).await?;
    let b = load(&state, id).await?;
    if !["booked", "confirmed", "waitlist"].contains(&b.status.as_str()) || b.starts_at <= Utc::now() {
        return Err(AppError::bad("this booking can no longer be changed"));
    }
    let new = match a.action.as_str() {
        "confirm" if b.status == "booked" => "confirmed",
        "confirm" if b.status == "confirmed" => return Ok(Json(client_view(&state, id).await?)),
        "cancel" => "cancelled",
        _ => return Err(AppError::bad("confirm or cancel")),
    };
    sqlx::query!("UPDATE bookings SET status = $2, updated_at = now() WHERE id = $1", id, new).execute(&state.db).await?;
    for item in [b.contact_id, b.deal_id].into_iter().flatten() {
        let _ = crate::crm::timeline(&state.db, item, "booking", None, None, json!({ "event": format!("client_{new}"), "booking_id": id, "number": b.number, "starts_at": b.starts_at })).await;
    }
    super::bookings::crm_event(&state, id, new).await;
    let people: Vec<Uuid> = sqlx::query_scalar!(r#"SELECT r.user_id AS "user_id!" FROM booking_resources r WHERE r.id = ANY($1) AND r.user_id IS NOT NULL"#, &b.resource_ids).fetch_all(&state.db).await?;
    crate::notify::notify(&state, &people, crate::notify::NewNotification {
        kind: "booking.client",
        title: format!("{} {} booking #{}", b.client_name, if new == "cancelled" { "cancelled" } else { "confirmed" }, b.number),
        body: None,
        link: Some(format!("/booking?booking={id}")),
        actor: None,
    })
    .await;
    if new == "cancelled" && b.status != "waitlist" {
        freed(&state, &b).await;
    }
    crate::realtime::publish(&state, None, "booking.changed", json!({ "id": id })).await;
    Ok(Json(client_view(&state, id).await?))
}
