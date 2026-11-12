//! Field visits (server side; a mobile app uses the same endpoints later).
//!
//! A visit is a CRM activity of kind `visit` at an address — the record's own, its company's
//! or its contact's, or one given — with a duration and either a fixed time or "any time that
//! day". **Planning a day** keeps fixed visits where they are and puts the others in the
//! shortest route from the start point (nearest neighbour, then 2-opt), timed around the fixed
//! ones with travel at the configured speed. **Check-in** records when and where (and how far
//! from the address); **check-out** completes the visit and logs it on the record.
//!
//! Coordinates come with the visit, from the address through the configured geocoder
//! (`GEOCODER_URL`, a Nominatim-compatible search API, e.g. a self-hosted Nominatim; results
//! are cached), or not at all — visits without them are grouped by postcode and city.
use std::collections::HashMap;

use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Duration, NaiveDate, NaiveTime, TimeZone, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{
    auth::AuthUser,
    crm::{access, timeline},
    error::{AppError, AppResult},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/field/visits", post(create))
        .route("/field/visits/{id}/check-in", post(check_in))
        .route("/field/visits/{id}/check-out", post(check_out))
        .route("/field/day", get(day))
        .route("/field/day/plan", post(plan))
        .route("/field/settings", get(get_settings).put(put_settings))
}

// ---- geometry ----

/// Kilometres between two points (haversine).
pub fn km(a: (f64, f64), b: (f64, f64)) -> f64 {
    let r = 6371.0;
    let (la1, la2) = (a.0.to_radians(), b.0.to_radians());
    let dla = la2 - la1;
    let dlo = (b.1 - a.1).to_radians();
    let h = (dla / 2.0).sin().powi(2) + la1.cos() * la2.cos() * (dlo / 2.0).sin().powi(2);
    2.0 * r * h.sqrt().asin()
}

/// An open path through the points from `start`: nearest neighbour, then 2-opt.
pub fn route(start: Option<(f64, f64)>, pts: &[(f64, f64)]) -> Vec<usize> {
    if pts.is_empty() {
        return vec![];
    }
    let mut left: Vec<usize> = (0..pts.len()).collect();
    let mut path = vec![];
    let mut at = match start {
        Some(s) => s,
        None => {
            let first = left.remove(0);
            path.push(first);
            pts[first]
        }
    };
    while !left.is_empty() {
        let (i, _) = left.iter().enumerate().min_by(|x, y| km(at, pts[*x.1]).partial_cmp(&km(at, pts[*y.1])).unwrap_or(std::cmp::Ordering::Equal)).expect("not empty");
        let next = left.remove(i);
        path.push(next);
        at = pts[next];
    }
    // 2-opt on the open path (the start point fixed in front).
    let cost = |p: &[usize]| -> f64 {
        let mut c = 0.0;
        let mut prev = start;
        for &i in p {
            if let Some(q) = prev {
                c += km(q, pts[i]);
            }
            prev = Some(pts[i]);
        }
        c
    };
    let mut best = cost(&path);
    let mut improved = true;
    let mut rounds = 0;
    while improved && rounds < 50 {
        improved = false;
        rounds += 1;
        for i in 0..path.len() {
            for j in i + 1..path.len() {
                let mut cand = path.clone();
                cand[i..=j].reverse();
                let c = cost(&cand);
                if c + 1e-9 < best {
                    best = c;
                    path = cand;
                    improved = true;
                }
            }
        }
    }
    path
}

// ---- settings ----

#[derive(serde::Serialize, Deserialize, Clone)]
struct FieldSettings {
    #[serde(default = "speed")]
    speed_kmh: f64,
    #[serde(default)]
    start_lat: Option<f64>,
    #[serde(default)]
    start_lng: Option<f64>,
    #[serde(default)]
    start_label: Option<String>,
    #[serde(default = "nine")]
    day_starts: String,
}
fn speed() -> f64 {
    40.0
}
fn nine() -> String {
    "09:00".into()
}
impl Default for FieldSettings {
    fn default() -> Self {
        FieldSettings { speed_kmh: 40.0, start_lat: None, start_lng: None, start_label: None, day_starts: nine() }
    }
}

async fn settings(state: &AppState) -> FieldSettings {
    sqlx::query_scalar::<_, Value>("SELECT value FROM settings WHERE key = 'field_visits'")
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default()
}

async fn get_settings(State(state): State<AppState>, _auth: AuthUser) -> AppResult<Json<Value>> {
    Ok(Json(json!({ "settings": settings(&state).await, "geocoder": geocoder().is_some() })))
}

async fn put_settings(State(state): State<AppState>, auth: AuthUser, Json(b): Json<FieldSettings>) -> AppResult<Json<Value>> {
    if !access::is_crm_admin(&auth) {
        return Err(AppError::Forbidden);
    }
    if !(5.0..=200.0).contains(&b.speed_kmh) || NaiveTime::parse_from_str(&b.day_starts, "%H:%M").is_err() {
        return Err(AppError::bad("speed between 5 and 200 km/h, and the day's start as HH:MM"));
    }
    if b.start_lat.is_some_and(|l| !(-90.0..=90.0).contains(&l)) || b.start_lng.is_some_and(|l| !(-180.0..=180.0).contains(&l)) {
        return Err(AppError::bad("the start point's coordinates are out of range"));
    }
    sqlx::query("INSERT INTO settings (key, value) VALUES ('field_visits', $1) ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = now()")
        .bind(json!(b))
        .execute(&state.db)
        .await?;
    Ok(Json(json!({ "settings": b })))
}

// ---- geocoding ----

fn geocoder() -> Option<String> {
    std::env::var("GEOCODER_URL").ok().filter(|s| !s.trim().is_empty()).map(|s| s.trim_end_matches('/').to_string())
}

fn address_text(a: &Value) -> String {
    ["line1", "city", "postal_code", "region", "country"].iter().filter_map(|k| a.get(*k).and_then(Value::as_str)).map(str::trim).filter(|s| !s.is_empty()).collect::<Vec<_>>().join(", ")
}

async fn geocode(state: &AppState, address: &Value) -> Option<(f64, f64)> {
    let q = address_text(address);
    if q.len() < 3 {
        return None;
    }
    let cached: Option<(Option<f64>, Option<f64>)> = sqlx::query_as("SELECT lat, lng FROM geo_cache WHERE query = $1").bind(&q).fetch_optional(&state.db).await.ok().flatten();
    if let Some((lat, lng)) = cached {
        return lat.zip(lng);
    }
    let base = geocoder()?;
    let tls = crate::crm::email::imap::client_config(false).ok()?;
    let client = reqwest::Client::builder().tls_backend_preconfigured(tls).timeout(std::time::Duration::from_secs(10)).user_agent("NERP").build().ok()?;
    let found: Option<(f64, f64)> = async {
        let url = reqwest::Url::parse_with_params(&format!("{base}/search"), &[("format", "json"), ("limit", "1"), ("q", q.as_str())]).ok()?;
        let r = client.get(url).send().await.ok()?;
        let v: Value = r.json().await.ok()?;
        let first = v.as_array()?.first()?.clone();
        let p = |k: &str| first.get(k).and_then(|x| x.as_str().and_then(|s| s.parse::<f64>().ok()).or_else(|| x.as_f64()));
        Some((p("lat")?, p("lon")?))
    }
    .await;
    let _ = sqlx::query("INSERT INTO geo_cache (query, lat, lng) VALUES ($1, $2, $3) ON CONFLICT (query) DO UPDATE SET lat = EXCLUDED.lat, lng = EXCLUDED.lng, created_at = now()")
        .bind(&q)
        .bind(found.map(|f| f.0))
        .bind(found.map(|f| f.1))
        .execute(&state.db)
        .await;
    found
}

// ---- visits ----

async fn company_tz(state: &AppState) -> chrono_tz::Tz {
    let v: Option<Value> = sqlx::query_scalar("SELECT value FROM settings WHERE key = 'default_timezone'").fetch_optional(&state.db).await.ok().flatten();
    v.and_then(|v| v.as_str().and_then(|s| s.parse().ok())).unwrap_or(chrono_tz::UTC)
}

fn at(tz: chrono_tz::Tz, d: NaiveDate, t: NaiveTime) -> DateTime<Utc> {
    tz.from_local_datetime(&d.and_time(t)).earliest().map(|x| x.with_timezone(&Utc)).unwrap_or_else(|| Utc.from_utc_datetime(&d.and_time(t)))
}

#[derive(Deserialize)]
struct NewVisit {
    item_id: Uuid,
    day: NaiveDate,
    /// HH:MM for a fixed time; none for "any time that day".
    time: Option<String>,
    #[serde(default = "thirty")]
    duration_minutes: i32,
    subject: Option<String>,
    responsible_id: Option<Uuid>,
    address: Option<Value>,
    lat: Option<f64>,
    lng: Option<f64>,
}
fn thirty() -> i32 {
    30
}

/// The record's address, else its company's, else its first contact's.
async fn address_of(state: &AppState, item: Uuid) -> AppResult<Option<Value>> {
    Ok(sqlx::query_scalar(
        "SELECT COALESCE(NULLIF(i.address, 'null'::jsonb), co.address,
                (SELECT c.address FROM crm_item_contacts ic JOIN crm_items c ON c.id = ic.contact_id WHERE ic.item_id = i.id AND c.address IS NOT NULL ORDER BY ic.is_primary DESC, ic.sort LIMIT 1))
         FROM crm_items i LEFT JOIN crm_items co ON co.id = i.company_id WHERE i.id = $1",
    )
    .bind(item)
    .fetch_optional(&state.db)
    .await?
    .flatten())
}

async fn create(State(state): State<AppState>, auth: AuthUser, Json(b): Json<NewVisit>) -> AppResult<Json<Value>> {
    let r = crate::crm::items::load_row(&state, b.item_id, false).await?;
    access::require_read(&state, &auth, r.entity_type, b.item_id, r.assigned_to).await?;
    if !(5..=720).contains(&b.duration_minutes) {
        return Err(AppError::bad("a visit lasts between 5 minutes and 12 hours"));
    }
    if b.lat.is_some_and(|l| !(-90.0..=90.0).contains(&l)) || b.lng.is_some_and(|l| !(-180.0..=180.0).contains(&l)) {
        return Err(AppError::bad("coordinates are out of range"));
    }
    let tz = company_tz(&state).await;
    let s = settings(&state).await;
    let fixed = b.time.as_deref().filter(|t| !t.is_empty()).map(|t| NaiveTime::parse_from_str(t, "%H:%M").map_err(|_| AppError::bad("the time is HH:MM"))).transpose()?;
    let start = at(tz, b.day, fixed.unwrap_or_else(|| NaiveTime::parse_from_str(&s.day_starts, "%H:%M").unwrap_or(NaiveTime::from_hms_opt(9, 0, 0).expect("9:00"))));
    let end = start + Duration::minutes(b.duration_minutes as i64);
    let address = match b.address.filter(|a| a.is_object()) {
        Some(a) => Some(a),
        None => address_of(&state, b.item_id).await?,
    };
    let coords = match (b.lat, b.lng) {
        (Some(la), Some(lo)) => Some((la, lo)),
        _ => match &address {
            Some(a) => match (a.get("lat").and_then(Value::as_f64), a.get("lng").and_then(Value::as_f64)) {
                (Some(la), Some(lo)) => Some((la, lo)),
                _ => geocode(&state, a).await,
            },
            None => None,
        },
    };
    let subject = b.subject.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).unwrap_or_else(|| format!("Visit: {}", r.title));
    let who = b.responsible_id.unwrap_or(auth.id);
    let mut tx = state.db.begin().await?;
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO crm_activities (item_id, kind, subject, starts_at, ends_at, deadline, responsible_id, created_by) VALUES ($1, 'visit', $2, $3, $4, $3, $5, $6) RETURNING id",
    )
    .bind(b.item_id)
    .bind(&subject)
    .bind(start)
    .bind(end)
    .bind(who)
    .bind(auth.id)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO field_visits (activity_id, address, lat, lng, duration_minutes, fixed_time) VALUES ($1, $2, $3, $4, $5, $6)")
        .bind(id)
        .bind(&address)
        .bind(coords.map(|c| c.0))
        .bind(coords.map(|c| c.1))
        .bind(b.duration_minutes)
        .bind(fixed.is_some())
        .execute(&mut *tx)
        .await?;
    timeline(&mut *tx, b.item_id, "activity", Some(auth.id), Some(subject.clone()), json!({ "activity_id": id, "kind": "visit", "event": "created" })).await?;
    tx.commit().await?;
    Ok(Json(json!({ "id": id, "starts_at": start, "fixed_time": fixed.is_some(), "has_coordinates": coords.is_some() })))
}

#[derive(sqlx::FromRow, Clone)]
struct V {
    id: Uuid,
    item_id: Uuid,
    code: String,
    title: String,
    subject: String,
    starts_at: Option<DateTime<Utc>>,
    ends_at: Option<DateTime<Utc>>,
    is_completed: bool,
    address: Option<Value>,
    lat: Option<f64>,
    lng: Option<f64>,
    duration_minutes: i32,
    fixed_time: bool,
    route_order: Option<i32>,
    checked_in_at: Option<DateTime<Utc>>,
    in_distance_m: Option<i32>,
    checked_out_at: Option<DateTime<Utc>>,
    notes: Option<String>,
}

async fn visits_of(state: &AppState, user: Uuid, day: NaiveDate) -> AppResult<Vec<V>> {
    let tz = company_tz(state).await;
    let (from, to) = (at(tz, day, NaiveTime::MIN), at(tz, day + Duration::days(1), NaiveTime::MIN));
    Ok(sqlx::query_as(
        "SELECT a.id, a.item_id, t.code, i.title, a.subject, a.starts_at, a.ends_at, a.is_completed, v.address, v.lat, v.lng, v.duration_minutes, v.fixed_time,
                v.route_order, v.checked_in_at, v.in_distance_m, v.checked_out_at, v.notes
         FROM crm_activities a JOIN field_visits v ON v.activity_id = a.id JOIN crm_items i ON i.id = a.item_id JOIN crm_entity_types t ON t.id = i.entity_type
         WHERE a.kind = 'visit' AND a.responsible_id = $1 AND a.starts_at >= $2 AND a.starts_at < $3 AND i.deleted_at IS NULL
         ORDER BY a.starts_at, v.route_order NULLS LAST",
    )
    .bind(user)
    .bind(from)
    .bind(to)
    .fetch_all(&state.db)
    .await?)
}

/// Whose day the caller may see: their own, or their team's (heads), or anyone's (CRM admins).
async fn may_see(state: &AppState, auth: &AuthUser, user: Uuid) -> AppResult<()> {
    if user == auth.id || access::is_crm_admin(auth) {
        return Ok(());
    }
    if crate::hr::manages(state, auth, user).await? { Ok(()) } else { Err(AppError::Forbidden) }
}

#[derive(Deserialize)]
struct DayQ {
    date: NaiveDate,
    user: Option<Uuid>,
}

fn day_json(vs: &[V], s: &FieldSettings) -> Value {
    let start = s.start_lat.zip(s.start_lng);
    let mut prev = start;
    let mut total_km = 0.0;
    let items: Vec<Value> = vs
        .iter()
        .map(|v| {
            let here = v.lat.zip(v.lng);
            let leg = match (prev, here) {
                (Some(p), Some(h)) => Some(km(p, h)),
                _ => None,
            };
            if let Some(l) = leg {
                total_km += l;
            }
            if here.is_some() {
                prev = here;
            }
            json!({
                "id": v.id, "item_id": v.item_id, "title": v.title, "subject": v.subject, "link": format!("/crm/{}/{}", v.code, v.item_id),
                "order": v.route_order, "starts_at": v.starts_at, "ends_at": v.ends_at, "duration_minutes": v.duration_minutes, "fixed_time": v.fixed_time, "done": v.is_completed,
                "address": v.address.as_ref().map(address_text), "lat": v.lat, "lng": v.lng,
                "leg_km": leg.map(|l| (l * 10.0).round() / 10.0), "leg_minutes": leg.map(|l| (l / s.speed_kmh * 60.0).round() as i64),
                "checked_in_at": v.checked_in_at, "in_distance_m": v.in_distance_m, "checked_out_at": v.checked_out_at, "notes": v.notes,
            })
        })
        .collect();
    json!({
        "visits": items, "total_km": (total_km * 10.0).round() / 10.0, "travel_minutes": (total_km / s.speed_kmh * 60.0).round() as i64,
        "start": start.map(|(a, b)| json!({ "lat": a, "lng": b, "label": s.start_label })),
    })
}

async fn day(State(state): State<AppState>, auth: AuthUser, Query(q): Query<DayQ>) -> AppResult<Json<Value>> {
    let user = q.user.unwrap_or(auth.id);
    may_see(&state, &auth, user).await?;
    let vs = visits_of(&state, user, q.date).await?;
    Ok(Json(day_json(&vs, &settings(&state).await)))
}

#[derive(Deserialize)]
struct PlanInput {
    date: NaiveDate,
    user: Option<Uuid>,
    /// Where the day starts (default: the configured start point).
    start_lat: Option<f64>,
    start_lng: Option<f64>,
    /// HH:MM (default: the configured start of the day).
    starts: Option<String>,
}

/// Orders the day: fixed visits stay; the others follow the shortest route from the start
/// and are timed around the fixed ones.
async fn plan(State(state): State<AppState>, auth: AuthUser, Json(b): Json<PlanInput>) -> AppResult<Json<Value>> {
    let user = b.user.unwrap_or(auth.id);
    may_see(&state, &auth, user).await?;
    let s = settings(&state).await;
    let tz = company_tz(&state).await;
    let mut vs = visits_of(&state, user, b.date).await?;
    // Coordinates for those that have an address but none yet.
    for v in vs.iter_mut().filter(|v| v.lat.is_none() && !v.is_completed) {
        if let Some(a) = &v.address {
            if let Some((la, lo)) = geocode(&state, a).await {
                v.lat = Some(la);
                v.lng = Some(lo);
                sqlx::query("UPDATE field_visits SET lat = $2, lng = $3 WHERE activity_id = $1").bind(v.id).bind(la).bind(lo).execute(&state.db).await?;
            }
        }
    }
    let start = b.start_lat.zip(b.start_lng).or(s.start_lat.zip(s.start_lng));
    let day_start = NaiveTime::parse_from_str(b.starts.as_deref().unwrap_or(&s.day_starts), "%H:%M").map_err(|_| AppError::bad("the start is HH:MM"))?;
    let fixed: Vec<V> = vs.iter().filter(|v| v.fixed_time || v.is_completed || v.checked_in_at.is_some()).cloned().collect();
    let flexible: Vec<V> = vs.iter().filter(|v| !(v.fixed_time || v.is_completed || v.checked_in_at.is_some())).cloned().collect();
    let (located, unlocated): (Vec<V>, Vec<V>) = flexible.into_iter().partition(|v| v.lat.is_some() && v.lng.is_some());
    let order = route(start, &located.iter().map(|v| (v.lat.unwrap_or(0.0), v.lng.unwrap_or(0.0))).collect::<Vec<_>>());
    let mut sequence: Vec<V> = order.into_iter().map(|i| located[i].clone()).collect();
    let mut rest = unlocated;
    let key = |v: &V| -> (String, String) {
        let g = |k: &str| v.address.as_ref().and_then(|a| a.get(k)).and_then(Value::as_str).unwrap_or("").to_lowercase();
        (g("postal_code"), g("city"))
    };
    rest.sort_by_key(key);
    sequence.extend(rest);
    // Time them in turn, stepping past fixed visits.
    let mut busy: Vec<(DateTime<Utc>, DateTime<Utc>, Option<(f64, f64)>)> =
        fixed.iter().filter_map(|v| v.starts_at.map(|s0| (s0, v.ends_at.unwrap_or(s0 + Duration::minutes(v.duration_minutes as i64)), v.lat.zip(v.lng)))).collect();
    busy.sort_by_key(|b| b.0);
    let mut cursor = at(tz, b.date, day_start);
    let mut here = start;
    let mut planned: HashMap<Uuid, (DateTime<Utc>, DateTime<Utc>)> = HashMap::new();
    for v in &sequence {
        let pos = v.lat.zip(v.lng);
        loop {
            let travel = match (here, pos) {
                (Some(a), Some(p)) => Duration::minutes((km(a, p) / s.speed_kmh * 60.0).ceil() as i64),
                _ => Duration::zero(),
            };
            let st = cursor + travel;
            let en = st + Duration::minutes(v.duration_minutes as i64);
            match busy.iter().find(|f| st < f.1 && en > f.0) {
                Some(f) => {
                    cursor = f.1;
                    here = f.2.or(here);
                }
                None => {
                    planned.insert(v.id, (st, en));
                    cursor = en;
                    here = pos.or(here);
                    break;
                }
            }
        }
    }
    let mut tx = state.db.begin().await?;
    for (id, (st, en)) in &planned {
        sqlx::query("UPDATE crm_activities SET starts_at = $2, ends_at = $3, deadline = $2, updated_at = now() WHERE id = $1").bind(id).bind(st).bind(en).execute(&mut *tx).await?;
    }
    // The order of the whole day, by time.
    let mut all: Vec<(Uuid, DateTime<Utc>)> = vs.iter().map(|v| (v.id, planned.get(&v.id).map(|p| p.0).or(v.starts_at).unwrap_or(cursor))).collect();
    all.sort_by_key(|a| a.1);
    for (i, (id, _)) in all.iter().enumerate() {
        sqlx::query("UPDATE field_visits SET route_order = $2 WHERE activity_id = $1").bind(id).bind(i as i32 + 1).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    let vs = visits_of(&state, user, b.date).await?;
    Ok(Json(day_json(&vs, &s)))
}

#[derive(Deserialize)]
struct Where {
    lat: Option<f64>,
    lng: Option<f64>,
    accuracy_m: Option<f64>,
    notes: Option<String>,
}

async fn own_visit(state: &AppState, auth: &AuthUser, id: Uuid) -> AppResult<(Uuid, Option<f64>, Option<f64>, Option<DateTime<Utc>>, Option<DateTime<Utc>>, String)> {
    let row: Option<(Uuid, Uuid, Option<f64>, Option<f64>, Option<DateTime<Utc>>, Option<DateTime<Utc>>, String)> = sqlx::query_as(
        "SELECT a.item_id, a.responsible_id, v.lat, v.lng, v.checked_in_at, v.checked_out_at, a.subject FROM crm_activities a JOIN field_visits v ON v.activity_id = a.id WHERE a.id = $1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?;
    let (item, who, lat, lng, cin, cout, subject) = row.ok_or(AppError::NotFound("visit"))?;
    if who != auth.id {
        return Err(AppError::Forbidden);
    }
    Ok((item, lat, lng, cin, cout, subject))
}

fn check_point(w: &Where) -> AppResult<Option<(f64, f64)>> {
    match (w.lat, w.lng) {
        (Some(a), Some(b)) if (-90.0..=90.0).contains(&a) && (-180.0..=180.0).contains(&b) => Ok(Some((a, b))),
        (None, None) => Ok(None),
        _ => Err(AppError::bad("coordinates are out of range")),
    }
}

/// Arrived: when and where, and how far from the visit's address.
async fn check_in(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(w): Json<Where>) -> AppResult<Json<Value>> {
    let (item, lat, lng, cin, _, subject) = own_visit(&state, &auth, id).await?;
    if cin.is_some() {
        return Err(AppError::bad("already checked in"));
    }
    let p = check_point(&w)?;
    let dist = match (p, lat.zip(lng)) {
        (Some(p), Some(v)) => Some((km(p, v) * 1000.0).round() as i32),
        _ => None,
    };
    sqlx::query("UPDATE field_visits SET checked_in_at = now(), in_lat = $2, in_lng = $3, in_accuracy_m = $4, in_distance_m = $5 WHERE activity_id = $1")
        .bind(id)
        .bind(p.map(|x| x.0))
        .bind(p.map(|x| x.1))
        .bind(w.accuracy_m)
        .bind(dist)
        .execute(&state.db)
        .await?;
    timeline(&state.db, item, "activity", Some(auth.id), Some(subject), json!({ "activity_id": id, "kind": "visit", "event": "checked_in", "distance_m": dist })).await?;
    Ok(Json(json!({ "checked_in": true, "distance_m": dist, "far": dist.is_some_and(|d| d > 500) })))
}

/// Done: the visit is completed and logged on the record with its notes.
async fn check_out(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(w): Json<Where>) -> AppResult<Json<Value>> {
    let (item, _, _, cin, cout, subject) = own_visit(&state, &auth, id).await?;
    let cin = cin.ok_or_else(|| AppError::bad("check in first"))?;
    if cout.is_some() {
        return Err(AppError::bad("already checked out"));
    }
    let p = check_point(&w)?;
    let notes = w.notes.as_deref().map(str::trim).filter(|n| !n.is_empty()).map(|n| n.chars().take(5000).collect::<String>());
    let mut tx = state.db.begin().await?;
    sqlx::query("UPDATE field_visits SET checked_out_at = now(), out_lat = $2, out_lng = $3, notes = $4 WHERE activity_id = $1")
        .bind(id)
        .bind(p.map(|x| x.0))
        .bind(p.map(|x| x.1))
        .bind(&notes)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE crm_activities SET is_completed = TRUE, completed_at = now(), result = $2, updated_at = now() WHERE id = $1").bind(id).bind(&notes).execute(&mut *tx).await?;
    let minutes = (Utc::now() - cin).num_minutes();
    timeline(&mut *tx, item, "activity", Some(auth.id), Some(format!("{subject}: {minutes} min on site{}", notes.as_deref().map(|n| format!(". {n}")).unwrap_or_default())),
        json!({ "activity_id": id, "kind": "visit", "event": "completed", "minutes": minutes })).await?;
    sqlx::query("UPDATE crm_items SET last_activity_at = now() WHERE id = $1").bind(item).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(json!({ "done": true, "minutes": minutes })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distances() {
        // London to Paris is about 344 km.
        let d = km((51.5074, -0.1278), (48.8566, 2.3522));
        assert!((d - 344.0).abs() < 5.0, "{d}");
    }

    #[test]
    fn routes_follow_the_line() {
        // Points along a line, shuffled: the route visits them in order from the start.
        let pts = vec![(0.0, 3.0), (0.0, 1.0), (0.0, 4.0), (0.0, 2.0)];
        let r = route(Some((0.0, 0.0)), &pts);
        assert_eq!(r, vec![1, 3, 0, 2]);
    }
}
