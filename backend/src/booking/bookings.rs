//! Bookings: making, moving and changing them, the waiting list, free slots, the client's
//! CRM contact (and deal), emails to the client and reminders.

use anyhow::Result;
use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{free_slots, load_resource, load_services, overlapping, within_hours, Resource, Service};
use crate::{
    auth::{tokens::random_token, AuthUser},
    crm::{automation::system_auth, entity_type_by_id, items::{create_item, normalize}, CONTACT, DEAL},
    error::{AppError, AppResult},
    notify::{notify, NewNotification},
    routes::clean,
    state::AppState,
};

pub const REMIND_JOB: &str = "booking.remind";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/booking/bookings", get(list).post(create))
        .route("/booking/bookings/{id}", get(detail).patch(update))
        .route("/booking/bookings/{id}/status", post(set_status))
        .route("/booking/slots", get(slots))
        .route("/booking/waitlist", get(waitlist))
}

// ---------------------------------------------------------------------------
// Shapes
// ---------------------------------------------------------------------------

pub(crate) const BOOKING_SELECT: &str = "SELECT b.id, b.number, b.starts_at, b.ends_at, b.status, b.service_id, s.name AS service_name, b.extra_ids,
    b.price::text AS price, b.client_name, b.client_email, b.client_phone, b.contact_id, b.deal_id, b.note, b.source, b.overbooked,
    b.created_by, trim(u.first_name || ' ' || u.last_name) AS created_by_name, b.created_at,
    ARRAY(SELECT r.resource_id FROM booking_resources_used r WHERE r.booking_id = b.id) AS resource_ids
    FROM bookings b LEFT JOIN booking_services s ON s.id = b.service_id LEFT JOIN users u ON u.id = b.created_by";

#[derive(sqlx::FromRow, serde::Serialize)]
pub(crate) struct Booking {
    pub id: Uuid,
    pub number: i64,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
    pub status: String,
    pub service_id: Option<Uuid>,
    pub service_name: Option<String>,
    pub extra_ids: Vec<Uuid>,
    pub price: String,
    pub client_name: String,
    pub client_email: Option<String>,
    pub client_phone: Option<String>,
    pub contact_id: Option<Uuid>,
    pub deal_id: Option<Uuid>,
    pub note: Option<String>,
    pub source: String,
    pub overbooked: bool,
    pub created_by: Option<Uuid>,
    pub created_by_name: Option<String>,
    pub created_at: DateTime<Utc>,
    pub resource_ids: Vec<Uuid>,
}

pub(crate) async fn load(state: &AppState, id: Uuid) -> AppResult<Booking> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{BOOKING_SELECT} WHERE b.id = $1"))).bind(id).fetch_optional(&state.db).await?.ok_or(AppError::NotFound("booking"))
}

async fn tell(state: &AppState, id: Uuid) {
    crate::realtime::publish(state, None, "booking.changed", json!({ "id": id })).await;
}

// ---------------------------------------------------------------------------
// Listing
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct ListQuery {
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    /// Comma-separated resource ids.
    resources: Option<String>,
    contact_id: Option<Uuid>,
}

async fn list(State(state): State<AppState>, _auth: AuthUser, Query(q): Query<ListQuery>) -> AppResult<Json<Value>> {
    if q.to <= q.from || q.to - q.from > Duration::days(400) {
        return Err(AppError::bad("show at most 400 days at once"));
    }
    let res: Vec<Uuid> = q.resources.as_deref().unwrap_or("").split(',').filter_map(|s| s.parse().ok()).collect();
    let items: Vec<Booking> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{BOOKING_SELECT} WHERE b.status <> 'waitlist' AND b.starts_at < $2 AND b.ends_at > $1
           AND (cardinality($3::uuid[]) = 0 OR EXISTS (SELECT 1 FROM booking_resources_used r WHERE r.booking_id = b.id AND r.resource_id = ANY($3)))
           AND ($4::uuid IS NULL OR b.contact_id = $4)
         ORDER BY b.starts_at LIMIT 2000"
    )))
    .bind(q.from)
    .bind(q.to)
    .bind(&res)
    .bind(q.contact_id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({ "items": items })))
}

async fn waitlist(State(state): State<AppState>, _auth: AuthUser) -> AppResult<Json<Value>> {
    let items: Vec<Booking> = sqlx::query_as(sqlx::AssertSqlSafe(format!("{BOOKING_SELECT} WHERE b.status = 'waitlist' AND b.ends_at > now() ORDER BY b.created_at"))).fetch_all(&state.db).await?;
    Ok(Json(json!({ "items": items })))
}

async fn detail(State(state): State<AppState>, _auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Booking>> {
    Ok(Json(load(&state, id).await?))
}

#[derive(Deserialize)]
struct SlotsQuery {
    resource_id: Uuid,
    service_id: Option<Uuid>,
    minutes: Option<i64>,
    date: NaiveDate,
}

async fn slots(State(state): State<AppState>, _auth: AuthUser, Query(q): Query<SlotsQuery>) -> AppResult<Json<Value>> {
    let r = load_resource(&state, q.resource_id).await?;
    let minutes = match q.service_id {
        Some(s) => load_services(&state).await?.into_iter().find(|x| x.id == s).ok_or(AppError::NotFound("service"))?.duration_minutes as i64,
        None => q.minutes.unwrap_or(r.slot_minutes as i64),
    };
    Ok(Json(json!({ "slots": free_slots(&state, &r, Duration::minutes(minutes), q.date, false).await? })))
}

// ---------------------------------------------------------------------------
// Making a booking
// ---------------------------------------------------------------------------

#[derive(Deserialize, Default, Clone)]
pub(crate) struct NewBooking {
    pub resource_ids: Vec<Uuid>,
    pub service_id: Option<Uuid>,
    #[serde(default)]
    pub extra_ids: Vec<Uuid>,
    pub starts_at: DateTime<Utc>,
    /// Defaults to the service's duration.
    pub ends_at: Option<DateTime<Utc>>,
    pub client_name: String,
    pub client_email: Option<String>,
    pub client_phone: Option<String>,
    /// An existing CRM contact; otherwise one is found by email or made.
    pub contact_id: Option<Uuid>,
    pub note: Option<String>,
    /// Staff: go over capacity where the resource allows it.
    #[serde(default)]
    pub overbook: bool,
    /// Put it on the waiting list instead (nothing free at that time).
    #[serde(default)]
    pub waitlist: bool,
}

pub(crate) struct Made {
    pub booking: Booking,
    /// The client's link to see, confirm or cancel it.
    pub token: String,
}

/// Checks and stores a booking. `actor`: the employee making it (None: a client, through a
/// form). Resources must be free unless it goes on the waiting list or over capacity.
pub(crate) async fn make(state: &AppState, actor: Option<&AuthUser>, b: NewBooking, source: &str, form: Option<Uuid>) -> AppResult<Made> {
    if b.resource_ids.len() > 10 {
        return Err(AppError::bad("choose one to ten resources"));
    }
    let ids = b.resource_ids.clone();
    super::with_resource_locks(state, &ids, || make_locked(state, actor, b, source, form)).await
}

async fn make_locked(state: &AppState, actor: Option<&AuthUser>, b: NewBooking, source: &str, form: Option<Uuid>) -> AppResult<Made> {
    let name = clean(Some(b.client_name.clone())).ok_or_else(|| AppError::bad("who is it for?"))?;
    if name.chars().count() > 200 {
        return Err(AppError::bad("the name is too long"));
    }
    let email = clean(b.client_email.clone()).map(|e| e.to_lowercase());
    if email.as_deref().is_some_and(|e| !e.contains('@') || e.len() > 320) {
        return Err(AppError::bad("check the email address"));
    }
    let phone = clean(b.client_phone.clone()).map(|p| p.chars().take(40).collect::<String>());
    let mut res = b.resource_ids.clone();
    res.sort();
    res.dedup();
    if res.is_empty() || res.len() > 10 {
        return Err(AppError::bad("choose one to ten resources"));
    }
    let services = load_services(state).await?;
    let service: Option<&Service> = match b.service_id {
        Some(s) => Some(services.iter().find(|x| x.id == s && !x.is_extra && x.is_active).ok_or_else(|| AppError::bad("that service isn't offered"))?),
        None => None,
    };
    let extras: Vec<&Service> = b.extra_ids.iter().map(|e| services.iter().find(|x| x.id == *e && x.is_extra && x.is_active).ok_or_else(|| AppError::bad("an extra isn't offered"))).collect::<AppResult<_>>()?;
    let ends = match (b.ends_at, service) {
        (Some(e), _) => e,
        (None, Some(s)) => b.starts_at + Duration::minutes(s.duration_minutes as i64),
        (None, None) => return Err(AppError::bad("give the end time or choose a service")),
    };
    if ends <= b.starts_at {
        return Err(AppError::bad("the booking ends before it starts"));
    }
    if ends - b.starts_at > Duration::days(366) {
        return Err(AppError::bad("a booking lasts at most a year"));
    }
    let mut resources: Vec<Resource> = Vec::new();
    for id in &res {
        let r = load_resource(state, *id).await?;
        if !r.is_active {
            return Err(AppError::bad(format!("{} isn't taking bookings", r.name)));
        }
        if let Some(s) = service {
            if !s.resource_ids.is_empty() && !s.resource_ids.contains(id) {
                return Err(AppError::bad(format!("{} doesn't offer {}", r.name, s.name)));
            }
        }
        resources.push(r);
    }
    let mut overbooked = false;
    if !b.waitlist {
        for r in &resources {
            if !within_hours(state, r, b.starts_at, ends).await? && actor.is_none() {
                return Err(AppError::bad(format!("{} isn't available then", r.name)));
            }
            if overlapping(state, r, b.starts_at, ends, None).await? >= r.capacity as i64 {
                if actor.is_some() && b.overbook && r.allow_overbooking {
                    overbooked = true;
                } else {
                    return Err(AppError::Conflict(format!("{} is already booked then", r.name)));
                }
            }
        }
    }
    let price = service.map(|s| s.price).unwrap_or(Decimal::ZERO) + extras.iter().map(|e| e.price).sum::<Decimal>();

    // The client in the CRM: the contact given, or found by email, or made.
    let make_contact: bool = sqlx::query_scalar("SELECT make_contact FROM booking_settings WHERE id").fetch_one(&state.db).await?;
    let contact = match b.contact_id {
        Some(c) => Some(c),
        None if make_contact => find_or_make_contact(state, &name, email.as_deref(), phone.as_deref(), actor.map(|a| a.id)).await?,
        None => None,
    };
    let token = random_token(24);
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO bookings (starts_at, ends_at, status, service_id, extra_ids, price, client_name, client_email, client_phone, contact_id, note, source, form_id, overbooked, token_hash, created_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16) RETURNING id",
    )
    .bind(b.starts_at)
    .bind(ends)
    .bind(if b.waitlist { "waitlist" } else { "booked" })
    .bind(b.service_id)
    .bind(&b.extra_ids)
    .bind(price)
    .bind(&name)
    .bind(&email)
    .bind(&phone)
    .bind(contact)
    .bind(clean(b.note.clone()).map(|n| n.chars().take(4000).collect::<String>()))
    .bind(source)
    .bind(form)
    .bind(overbooked)
    .bind(hash(&token))
    .bind(actor.map(|a| a.id))
    .fetch_one(&state.db)
    .await?;
    for r in &res {
        sqlx::query("INSERT INTO booking_resources_used (booking_id, resource_id) VALUES ($1, $2)").bind(id).bind(r).execute(&state.db).await?;
    }
    let booking = load(state, id).await?;
    // A deal for it, when a pipeline is chosen.
    if !b.waitlist {
        if let Err(e) = make_deal(state, &booking, &resources, service).await {
            tracing::warn!(error = ?e, "booking: the deal could not be made");
        }
    }
    if let Some(c) = contact {
        let _ = crate::crm::timeline(&state.db, c, "booking", actor.map(|a| a.id), None, json!({ "event": if b.waitlist { "waitlist" } else { "booked" }, "booking_id": id, "number": booking.number, "starts_at": b.starts_at, "service": service.map(|s| s.name.clone()) })).await;
    }
    // The people booked are told.
    let people: Vec<Uuid> = resources.iter().filter_map(|r| r.user_id).filter(|u| Some(*u) != actor.map(|a| a.id)).collect();
    let when = b.starts_at.format("%Y-%m-%d %H:%M UTC");
    notify(state, &people, NewNotification {
        kind: "booking.new",
        title: if b.waitlist { format!("{name} is on the waiting list for {when}") } else { format!("New booking: {name}, {when}") },
        body: service.map(|s| s.name.clone()),
        link: Some(format!("/booking?booking={id}")),
        actor: actor.map(|a| a.id),
    })
    .await;
    if !b.waitlist {
        confirm_email(state, &booking, &token).await;
    }
    let booking = load(state, id).await?;
    tell(state, id).await;
    Ok(Made { booking, token })
}

/// Staff make a booking (or put someone on the waiting list).
async fn create(State(state): State<AppState>, auth: AuthUser, Json(b): Json<NewBooking>) -> AppResult<Json<Booking>> {
    Ok(Json(make(&state, Some(&auth), b, "manual", None).await?.booking))
}

pub(crate) fn hash(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

async fn find_or_make_contact(state: &AppState, name: &str, email: Option<&str>, phone: Option<&str>, by: Option<Uuid>) -> AppResult<Option<Uuid>> {
    for (kind, v) in [("email", email), ("phone", phone)] {
        let Some(v) = v else { continue };
        let found: Option<Uuid> = sqlx::query_scalar(
            "SELECT i.id FROM crm_multifields m JOIN crm_items i ON i.id = m.item_id
             WHERE m.kind = $1 AND m.normalized = $2 AND i.entity_type = $3 AND i.deleted_at IS NULL ORDER BY i.updated_at DESC LIMIT 1",
        )
        .bind(kind)
        .bind(normalize(kind, v))
        .bind(CONTACT)
        .fetch_optional(&state.db)
        .await?;
        if found.is_some() {
            return Ok(found);
        }
    }
    let mut parts = name.splitn(2, ' ');
    let mut c = Map::new();
    c.insert("first_name".into(), json!(parts.next().unwrap_or(name)));
    if let Some(l) = parts.next() {
        c.insert("last_name".into(), json!(l));
    }
    c.insert("source".into(), json!("booking"));
    c.insert("assigned_to".into(), json!(by));
    let t = entity_type_by_id(state, CONTACT).await?;
    // With the phone if the CRM accepts it, else without; a booking never fails for its contact.
    for with_phone in [true, false] {
        let mut mf = Vec::new();
        if let Some(e) = email {
            mf.push(json!({ "kind": "email", "value_type": "work", "value": e }));
        }
        if let Some(p) = phone.filter(|_| with_phone) {
            mf.push(json!({ "kind": "phone", "value_type": "mobile", "value": p }));
        }
        if !with_phone && phone.is_none() {
            break;
        }
        let mut body = c.clone();
        body.insert("multifields".into(), json!(mf));
        match create_item(state, &system_auth(), &t, body, None).await {
            Ok(id) => {
                crate::crm::broadcast(state, CONTACT, id, false).await;
                return Ok(Some(id));
            }
            Err(e) => tracing::info!(error = ?e, with_phone, "booking: contact not made"),
        }
    }
    Ok(None)
}

async fn make_deal(state: &AppState, b: &Booking, resources: &[Resource], service: Option<&Service>) -> AppResult<()> {
    let (cat, currency): (Option<Uuid>, String) = sqlx::query_as("SELECT deal_category_id, currency FROM booking_settings WHERE id").fetch_one(&state.db).await?;
    let Some(cat) = cat else { return Ok(()) };
    let t = entity_type_by_id(state, DEAL).await?;
    let mut d = Map::new();
    d.insert("title".into(), json!(format!("Booking #{}: {}{}", b.number, b.client_name, service.map(|s| format!(" — {}", s.name)).unwrap_or_default())));
    d.insert("category_id".into(), json!(cat));
    d.insert("currency".into(), json!(currency));
    d.insert("opportunity".into(), json!(b.price));
    d.insert("source".into(), json!("booking"));
    if let Some(c) = b.contact_id {
        d.insert("contact_ids".into(), json!([c]));
    }
    d.insert("assigned_to".into(), json!(resources.iter().find_map(|r| r.user_id).or(b.created_by)));
    let id = create_item(state, &system_auth(), &t, d, None).await?;
    sqlx::query("UPDATE bookings SET deal_id = $2 WHERE id = $1").bind(b.id).bind(id).execute(&state.db).await?;
    crate::crm::broadcast(state, DEAL, id, false).await;
    Ok(())
}

async fn company(state: &AppState) -> String {
    let v: Option<Value> = sqlx::query_scalar("SELECT value FROM settings WHERE key = 'company_name'").fetch_optional(&state.db).await.ok().flatten();
    v.and_then(|v| v.as_str().map(String::from)).unwrap_or_else(|| "NERP".into())
}

async fn what_and_when(state: &AppState, b: &Booking) -> (String, String) {
    let names: Vec<String> = sqlx::query_scalar("SELECT r.name FROM booking_resources_used u JOIN booking_resources r ON r.id = u.resource_id WHERE u.booking_id = $1 ORDER BY r.name")
        .bind(b.id)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();
    let tz: Option<String> = sqlx::query_scalar("SELECT r.timezone FROM booking_resources_used u JOIN booking_resources r ON r.id = u.resource_id WHERE u.booking_id = $1 LIMIT 1")
        .bind(b.id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten();
    let tz: chrono_tz::Tz = tz.and_then(|t| t.parse().ok()).unwrap_or(chrono_tz::UTC);
    let what = format!("{}{}", b.service_name.clone().unwrap_or_else(|| "Booking".into()), if names.is_empty() { String::new() } else { format!(" with {}", names.join(", ")) });
    (what, format!("{} ({})", b.starts_at.with_timezone(&tz).format("%A %e %B %Y, %H:%M"), tz.name()))
}

async fn confirm_email(state: &AppState, b: &Booking, token: &str) {
    let Some(to) = &b.client_email else { return };
    let on: bool = sqlx::query_scalar("SELECT confirm_email FROM booking_settings WHERE id").fetch_one(&state.db).await.unwrap_or(false);
    if !on {
        return;
    }
    let (what, when) = what_and_when(state, b).await;
    let name = company(state).await;
    let link = format!("{}/book/manage/{token}", state.config.public_url);
    let mail = crate::mail::customer_notice(to, &name, &format!("Your booking: {when}"), "You're booked", &[format!("Hi {},", b.client_name), format!("{what}, {when}. Booking #{}.", b.number), "You can confirm or cancel it here:".into()], Some(("See your booking", &link)));
    if let Err(e) = crate::jobs::send_email(state, mail).await {
        tracing::warn!(error = ?e, "booking: confirmation not queued");
    }
}

// ---------------------------------------------------------------------------
// Changing a booking
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct UpdateBooking {
    starts_at: Option<DateTime<Utc>>,
    ends_at: Option<DateTime<Utc>>,
    resource_ids: Option<Vec<Uuid>>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    note: Option<Option<String>>,
    client_name: Option<String>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    client_phone: Option<Option<String>>,
    #[serde(default)]
    overbook: bool,
}

/// Moves a booking (or places one from the waiting list), or changes its details.
async fn update(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<UpdateBooking>) -> AppResult<Json<Booking>> {
    let mut ids = load(&state, id).await?.resource_ids;
    ids.extend(b.resource_ids.iter().flatten().take(10));
    super::with_resource_locks(&state, &ids, || update_locked(&state, auth, id, b)).await
}

async fn update_locked(state: &AppState, auth: AuthUser, id: Uuid, b: UpdateBooking) -> AppResult<Json<Booking>> {
    let state = state.clone();
    let cur = load(&state, id).await?;
    if cur.status == "cancelled" {
        return Err(AppError::bad("a cancelled booking can't be changed"));
    }
    let starts = b.starts_at.unwrap_or(cur.starts_at);
    let ends = b.ends_at.unwrap_or_else(|| starts + (cur.ends_at - cur.starts_at));
    if ends <= starts {
        return Err(AppError::bad("the booking ends before it starts"));
    }
    let res = b.resource_ids.clone().unwrap_or(cur.resource_ids.clone());
    if res.is_empty() || res.len() > 10 {
        return Err(AppError::bad("choose one to ten resources"));
    }
    let moved = starts != cur.starts_at || ends != cur.ends_at || b.resource_ids.is_some();
    let mut overbooked = cur.overbooked;
    if moved && cur.status != "waitlist" {
        overbooked = false;
        for rid in &res {
            let r = load_resource(&state, *rid).await?;
            if overlapping(&state, &r, starts, ends, Some(id)).await? >= r.capacity as i64 {
                if b.overbook && r.allow_overbooking {
                    overbooked = true;
                } else {
                    return Err(AppError::Conflict(format!("{} is already booked then", r.name)));
                }
            }
        }
    }
    let mut tx = state.db.begin().await?;
    sqlx::query(
        "UPDATE bookings SET starts_at = $2, ends_at = $3, overbooked = $4, note = CASE WHEN $5 THEN $6 ELSE note END,
            client_name = COALESCE($7, client_name), client_phone = CASE WHEN $8 THEN $9 ELSE client_phone END,
            reminded_at = CASE WHEN $10 THEN NULL ELSE reminded_at END, updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(starts)
    .bind(ends)
    .bind(overbooked)
    .bind(b.note.is_some())
    .bind(clean(b.note.flatten()))
    .bind(clean(b.client_name))
    .bind(b.client_phone.is_some())
    .bind(clean(b.client_phone.flatten()))
    .bind(moved)
    .execute(&mut *tx)
    .await?;
    if let Some(r) = &b.resource_ids {
        sqlx::query("DELETE FROM booking_resources_used WHERE booking_id = $1").bind(id).execute(&mut *tx).await?;
        for x in r {
            sqlx::query("INSERT INTO booking_resources_used (booking_id, resource_id) SELECT $1, id FROM booking_resources WHERE id = $2").bind(id).bind(x).execute(&mut *tx).await?;
        }
    }
    tx.commit().await?;
    let _ = auth;
    tell(&state, id).await;
    Ok(Json(load(&state, id).await?))
}

#[derive(Deserialize)]
struct StatusBody {
    /// booked | confirmed | came | no_show | cancelled (a waiting-list booking becomes
    /// booked when it is placed, if its time is free).
    status: String,
}

async fn set_status(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<StatusBody>) -> AppResult<Json<Booking>> {
    if !["booked", "confirmed", "came", "no_show", "cancelled"].contains(&b.status.as_str()) {
        return Err(AppError::bad("unknown status"));
    }
    let cur = load(&state, id).await?;
    if cur.status == "waitlist" && b.status != "cancelled" {
        // Placing it: its time must be free now.
        for rid in &cur.resource_ids {
            let r = load_resource(&state, *rid).await?;
            if overlapping(&state, &r, cur.starts_at, cur.ends_at, Some(id)).await? >= r.capacity as i64 {
                return Err(AppError::Conflict(format!("{} is still booked then; move it to a free time", r.name)));
            }
        }
    }
    sqlx::query("UPDATE bookings SET status = $2, updated_at = now() WHERE id = $1").bind(id).bind(&b.status).execute(&state.db).await?;
    if let Some(c) = cur.contact_id {
        let _ = crate::crm::timeline(&state.db, c, "booking", Some(auth.id), None, json!({ "event": b.status, "booking_id": id, "number": cur.number, "starts_at": cur.starts_at })).await;
    }
    if b.status == "cancelled" && cur.status != "waitlist" {
        freed(&state, &cur).await;
    }
    tell(&state, id).await;
    Ok(Json(load(&state, id).await?))
}

/// A booking was cancelled: whoever waits for that time is told it is free.
pub(crate) async fn freed(state: &AppState, b: &Booking) {
    let waiting: Vec<(Uuid, i64, String, Option<Uuid>)> = sqlx::query_as(
        "SELECT DISTINCT w.id, w.number, w.client_name, w.created_by FROM bookings w JOIN booking_resources_used u ON u.booking_id = w.id
         WHERE w.status = 'waitlist' AND u.resource_id = ANY($1) AND w.starts_at < $3 AND w.ends_at > $2",
    )
    .bind(&b.resource_ids)
    .bind(b.starts_at)
    .bind(b.ends_at)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();
    if waiting.is_empty() {
        return;
    }
    let mut who: Vec<Uuid> = sqlx::query_scalar("SELECT r.user_id FROM booking_resources r WHERE r.id = ANY($1) AND r.user_id IS NOT NULL").bind(&b.resource_ids).fetch_all(&state.db).await.unwrap_or_default();
    who.extend(waiting.iter().filter_map(|w| w.3));
    who.sort();
    who.dedup();
    for (wid, n, name, _) in waiting {
        notify(state, &who, NewNotification {
            kind: "booking.freed",
            title: format!("A time {name} waits for is free (waiting list #{n})"),
            body: None,
            link: Some(format!("/booking?booking={wid}")),
            actor: None,
        })
        .await;
    }
}

// ---------------------------------------------------------------------------
// Reminders
// ---------------------------------------------------------------------------

/// Emails clients a reminder the set number of hours before (once). A new link goes with it.
pub async fn run_reminders(state: &AppState) -> Result<()> {
    let hours: i32 = sqlx::query_scalar("SELECT remind_hours FROM booking_settings WHERE id").fetch_one(&state.db).await?;
    if hours == 0 {
        return Ok(());
    }
    let due: Vec<Uuid> = sqlx::query_scalar(
        "UPDATE bookings SET reminded_at = now() WHERE status IN ('booked', 'confirmed') AND reminded_at IS NULL AND client_email IS NOT NULL
           AND starts_at > now() AND starts_at <= now() + make_interval(hours => $1) RETURNING id",
    )
    .bind(hours)
    .fetch_all(&state.db)
    .await?;
    let name = company(state).await;
    for id in due {
        let Ok(b) = load(state, id).await else { continue };
        let token = random_token(24);
        sqlx::query("UPDATE bookings SET token_hash = $2 WHERE id = $1").bind(id).bind(hash(&token)).execute(&state.db).await?;
        let (what, when) = what_and_when(state, &b).await;
        let link = format!("{}/book/manage/{token}", state.config.public_url);
        let mail = crate::mail::customer_notice(b.client_email.as_deref().unwrap(), &name, &format!("Reminder: {when}"), "See you soon", &[format!("Hi {},", b.client_name), format!("A reminder of your booking: {what}, {when}."), "Can't make it? Let us know:".into()], Some(("Confirm or cancel", &link)));
        crate::jobs::send_email(state, mail).await?;
    }
    Ok(())
}
