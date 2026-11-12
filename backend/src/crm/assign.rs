//! Capacity-aware assignment: the "assign" robot's `capacity` mode gives a record to whoever
//! of the chosen employees is in today, fits it, and has the least on their plate.
//!
//! - **In today:** not on approved time off, not a company holiday, and working today — by
//!   their rota when they have one this week, else the company's working week; optionally
//!   only those who started their working day (checked in) and haven't ended it.
//! - **Fits it:** when the rule names a field of the record (a language, a product line, a
//!   region…), people whose skills include its value come first; with nobody matching, the
//!   others still count unless the rule says skills are required.
//! - **Least on their plate:** open tasks plus open records of the same type; an optional cap
//!   skips anyone at or over it. Ties go round in turn.
//!
//! When nobody qualifies, the record goes to the least busy of all the chosen (or, if the rule
//! says so, stays with whoever has it). The preview endpoint shows the reasoning.
use std::collections::HashMap;

use axum::{
    extract::{Path, Query, State},
    routing::get,
    Json, Router,
};
use chrono::{Datelike, Duration, NaiveDate, Utc};
use redis::AsyncCommands;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{
    audit,
    auth::{permissions as perm, AuthUser},
    error::{AppError, AppResult},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/users/{id}/skills", get(get_skills).put(put_skills))
        .route("/crm/assign/preview", get(preview))
}

// ---- skills ----

async fn employee(state: &AppState, id: Uuid) -> AppResult<()> {
    let found: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users WHERE id = $1 AND NOT is_guest AND NOT is_system)").bind(id).fetch_one(&state.db).await?;
    if found { Ok(()) } else { Err(AppError::NotFound("employee")) }
}

async fn get_skills(State(state): State<AppState>, _auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    employee(&state, id).await?;
    let skills: Vec<String> = sqlx::query_scalar("SELECT skill FROM user_skills WHERE user_id = $1 ORDER BY lower(skill)").bind(id).fetch_all(&state.db).await?;
    Ok(Json(json!({ "skills": skills })))
}

#[derive(Deserialize)]
struct SkillsInput {
    skills: Vec<String>,
}

/// One's own skills, or anyone's for those who manage employees.
async fn put_skills(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<SkillsInput>) -> AppResult<Json<Value>> {
    if id != auth.id && !auth.can(perm::USERS_MANAGE) {
        return Err(AppError::Forbidden);
    }
    employee(&state, id).await?;
    let mut skills: Vec<String> = b.skills.iter().map(|s| s.split_whitespace().collect::<Vec<_>>().join(" ")).filter(|s| !s.is_empty()).collect();
    skills.sort_by_key(|s| s.to_lowercase());
    skills.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    if skills.len() > 50 || skills.iter().any(|s| s.chars().count() > 60) {
        return Err(AppError::bad("at most 50 skills of up to 60 characters"));
    }
    let mut tx = state.db.begin().await?;
    sqlx::query("DELETE FROM user_skills WHERE user_id = $1").bind(id).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO user_skills (user_id, skill) SELECT $1, unnest($2::text[])").bind(id).bind(&skills).execute(&mut *tx).await?;
    tx.commit().await?;
    if id != auth.id {
        audit::record(&state.db, Some(auth.id), "user.skills", "user", Some(id.to_string()), json!({ "skills": skills }), None).await;
    }
    Ok(Json(json!({ "skills": skills })))
}

// ---- who is in ----

async fn company_tz(state: &AppState) -> chrono_tz::Tz {
    let v: Option<Value> = sqlx::query_scalar("SELECT value FROM settings WHERE key = 'default_timezone'").fetch_optional(&state.db).await.ok().flatten();
    v.and_then(|v| v.as_str().and_then(|s| s.parse().ok())).unwrap_or(chrono_tz::UTC)
}

/// For each person: in today or not, and why.
pub async fn in_today(state: &AppState, users: &[Uuid], checked_in: bool) -> AppResult<HashMap<Uuid, (bool, String)>> {
    let today = Utc::now().with_timezone(&company_tz(state).await).date_naive();
    let week_start = today - Duration::days(today.weekday().num_days_from_monday() as i64);
    let holiday: Option<String> = sqlx::query_scalar("SELECT name FROM hr_holidays WHERE day = $1").bind(today).fetch_optional(&state.db).await?;
    let weekdays: Vec<i32> = sqlx::query_scalar("SELECT work_weekdays FROM hr_settings LIMIT 1").fetch_optional(&state.db).await?.unwrap_or(vec![1, 2, 3, 4, 5]);
    let away: Vec<(Uuid, String)> = sqlx::query_as("SELECT user_id, kind FROM hr_absences WHERE user_id = ANY($1) AND status = 'approved' AND starts_on <= $2 AND ends_on >= $2")
        .bind(users)
        .bind(today)
        .fetch_all(&state.db)
        .await?;
    let rota: Vec<(Uuid, NaiveDate)> = sqlx::query_as("SELECT user_id, day FROM hr_rota WHERE user_id = ANY($1) AND day BETWEEN $2 AND $3")
        .bind(users)
        .bind(week_start)
        .bind(week_start + Duration::days(6))
        .fetch_all(&state.db)
        .await?;
    let started: Vec<(Uuid, bool)> = sqlx::query_as("SELECT user_id, ended_at IS NULL FROM hr_work_days WHERE user_id = ANY($1) AND day = $2")
        .bind(users)
        .bind(today)
        .fetch_all(&state.db)
        .await?;
    let iso = today.weekday().number_from_monday() as i32;
    let mut out = HashMap::new();
    for u in users {
        let verdict = if let Some((_, kind)) = away.iter().find(|a| a.0 == *u) {
            (false, format!("away ({})", kind.replace('_', " ")))
        } else if let Some(h) = &holiday {
            (false, format!("holiday ({h})"))
        } else if rota.iter().any(|r| r.0 == *u) && !rota.iter().any(|r| r.0 == *u && r.1 == today) {
            (false, "not on the rota today".into())
        } else if !rota.iter().any(|r| r.0 == *u) && !weekdays.contains(&iso) {
            (false, "not a working day".into())
        } else if checked_in {
            match started.iter().find(|s| s.0 == *u) {
                Some((_, true)) => (true, "checked in".into()),
                Some((_, false)) => (false, "ended their day".into()),
                None => (false, "hasn't checked in".into()),
            }
        } else {
            (true, "working today".into())
        }
        ;
        out.insert(*u, verdict);
    }
    Ok(out)
}

/// The value of a record's field, as text values to match skills against.
async fn field_values(state: &AppState, item: Uuid, field: &str) -> AppResult<Vec<String>> {
    let v: Option<Value> = if let Some(code) = field.strip_prefix("uf.") {
        sqlx::query_scalar("SELECT uf -> $2 FROM crm_items WHERE id = $1").bind(item).bind(code).fetch_optional(&state.db).await?.flatten()
    } else if ["country", "city", "region"].contains(&field) {
        sqlx::query_scalar("SELECT address -> $2 FROM crm_items WHERE id = $1").bind(item).bind(field).fetch_optional(&state.db).await?.flatten()
    } else {
        sqlx::query_scalar("SELECT to_jsonb(i) -> $2 FROM crm_items i WHERE id = $1").bind(item).bind(field).fetch_optional(&state.db).await?.flatten()
    };
    let text = |v: &Value| match v {
        Value::String(s) => Some(s.trim().to_lowercase()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    };
    Ok(match v {
        Some(Value::Array(a)) => a.iter().filter_map(text).filter(|s| !s.is_empty()).collect(),
        Some(v) => text(&v).filter(|s| !s.is_empty()).into_iter().collect(),
        None => vec![],
    })
}

#[derive(Deserialize, Default, Clone)]
pub struct Options {
    /// A field of the record to match against people's skills.
    pub skill_field: Option<String>,
    /// Only people with a matching skill (else: matching people first).
    #[serde(default)]
    pub skills_required: bool,
    /// Only people who checked in today.
    #[serde(default)]
    pub checked_in: bool,
    /// Skip people with this many open tasks and records or more.
    pub max_open: Option<i64>,
    /// With nobody available: "least_busy" (default) or "keep".
    pub if_nobody: Option<String>,
}

/// Each candidate with why they would or wouldn't get the record; the pick first.
pub async fn evaluate(state: &AppState, entity_type: i16, item: Option<Uuid>, users: &[Uuid], o: &Options, turn: u64) -> AppResult<(Option<Uuid>, Vec<Value>, String)> {
    let active: Vec<(Uuid, String)> = sqlx::query_as("SELECT id, trim(first_name || ' ' || last_name) FROM users WHERE id = ANY($1) AND is_active AND NOT is_guest")
        .bind(users)
        .fetch_all(&state.db)
        .await?;
    let ids: Vec<Uuid> = users.iter().copied().filter(|u| active.iter().any(|a| a.0 == *u)).collect();
    let presence = in_today(state, &ids, o.checked_in).await?;
    let load: HashMap<Uuid, (i64, i64)> = sqlx::query_as::<_, (Uuid, i64, i64)>(
        "SELECT u, (SELECT COUNT(*) FROM tasks t WHERE t.responsible_id = u AND t.status IN ('new', 'in_progress', 'review')),
                   (SELECT COUNT(*) FROM crm_items i WHERE i.assigned_to = u AND i.entity_type = $2 AND i.deleted_at IS NULL AND NOT i.is_closed AND ($3::uuid IS NULL OR i.id <> $3))
         FROM unnest($1::uuid[]) AS u",
    )
    .bind(&ids)
    .bind(entity_type)
    .bind(item)
    .fetch_all(&state.db)
    .await?
    .into_iter()
    .map(|(u, t, r)| (u, (t, r)))
    .collect();
    let skills: Vec<(Uuid, String)> = sqlx::query_as("SELECT user_id, lower(skill) FROM user_skills WHERE user_id = ANY($1)").bind(&ids).fetch_all(&state.db).await?;
    let wanted = match (&o.skill_field, item) {
        (Some(f), Some(i)) if !f.is_empty() => field_values(state, i, f).await?,
        _ => vec![],
    };
    let fits = |u: &Uuid| !wanted.is_empty() && skills.iter().any(|(su, s)| su == u && wanted.contains(s));
    let any_fits = ids.iter().any(|u| presence.get(u).is_some_and(|p| p.0) && fits(u));
    // Who qualifies, in the chosen order.
    let mut rows = vec![];
    let mut eligible: Vec<(Uuid, i64)> = vec![];
    for u in &ids {
        let (t, r) = load.get(u).copied().unwrap_or((0, 0));
        let open = t + r;
        let (inn, why) = presence.get(u).cloned().unwrap_or((false, "unknown".into()));
        let mut reason = why.clone();
        let mut ok = inn;
        if ok && o.max_open.is_some_and(|m| open >= m) {
            ok = false;
            reason = format!("{open} open (the cap is {})", o.max_open.unwrap_or(0));
        }
        if ok && !wanted.is_empty() && !fits(u) && (o.skills_required || any_fits) {
            ok = false;
            reason = format!("no skill for {}", wanted.join(", "));
        }
        if ok {
            eligible.push((*u, open));
        }
        rows.push(json!({
            "id": u, "name": active.iter().find(|a| a.0 == *u).map(|a| a.1.clone()), "in_today": inn, "why": reason, "eligible": ok,
            "open_tasks": t, "open_records": r, "skills": skills.iter().filter(|s| s.0 == *u).map(|s| s.1.clone()).collect::<Vec<_>>(), "fits": fits(u),
        }));
    }
    let (pick, how) = if !eligible.is_empty() {
        let least = eligible.iter().map(|e| e.1).min().unwrap_or(0);
        let tied: Vec<Uuid> = eligible.iter().filter(|e| e.1 == least).map(|e| e.0).collect();
        let who = tied[(turn as usize) % tied.len()];
        (Some(who), format!("in today{}, fewest open ({least})", if any_fits { " with the skill" } else { "" }))
    } else if o.if_nobody.as_deref() == Some("keep") || ids.is_empty() {
        (None, "nobody available; left as it was".into())
    } else {
        let who = ids.iter().min_by_key(|u| load.get(u).map(|l| l.0 + l.1).unwrap_or(0)).copied();
        (who, "nobody available; the least busy of all".into())
    };
    for r in rows.iter_mut() {
        r["picked"] = json!(Some(r["id"].as_str().and_then(|s| s.parse::<Uuid>().ok())) == Some(pick));
    }
    rows.sort_by_key(|r| (!r["picked"].as_bool().unwrap_or(false), !r["eligible"].as_bool().unwrap_or(false)));
    Ok((pick, rows, how))
}

/// For the robot: the person to assign, taking the next turn among ties.
pub async fn pick(state: &AppState, robot: Uuid, entity_type: i16, item: Uuid, users: &[Uuid], o: &Options) -> AppResult<(Option<Uuid>, String)> {
    let mut redis = state.redis.clone();
    let turn: i64 = redis.incr(format!("crm:robot:cap:{robot}"), 1).await?;
    let (who, _, how) = evaluate(state, entity_type, Some(item), users, o, (turn.max(1) - 1) as u64).await?;
    Ok((who, how))
}

#[derive(Deserialize)]
struct PreviewQ {
    #[serde(rename = "type")]
    type_code: String,
    users: String,
    item: Option<Uuid>,
    skill_field: Option<String>,
    #[serde(default)]
    skills_required: bool,
    #[serde(default)]
    checked_in: bool,
    max_open: Option<i64>,
    if_nobody: Option<String>,
}

/// Who would get a record now, and why each of the others wouldn't.
async fn preview(State(state): State<AppState>, auth: AuthUser, Query(q): Query<PreviewQ>) -> AppResult<Json<Value>> {
    let t = super::entity_type(&state, &q.type_code).await?;
    super::access::require_any(&state, &auth, t.id, "read").await?;
    let users: Vec<Uuid> = q.users.split(',').filter_map(|s| s.trim().parse().ok()).take(100).collect();
    let o = Options { skill_field: q.skill_field, skills_required: q.skills_required, checked_in: q.checked_in, max_open: q.max_open, if_nobody: q.if_nobody };
    let (pick, people, how) = evaluate(&state, t.id, q.item, &users, &o, 0).await?;
    Ok(Json(json!({ "pick": pick, "how": how, "people": people })))
}

#[cfg(test)]
mod tests {
    #[test]
    fn options_default_to_lenient() {
        let o: super::Options = serde_json::from_str("{}").unwrap();
        assert!(!o.skills_required && !o.checked_in && o.max_open.is_none());
    }
}
