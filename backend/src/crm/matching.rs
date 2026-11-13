//! Duplicate prevention while typing: as a contact, lead or company is entered, what NERP
//! already knows about that person or business —
//!
//! - **CRM records of any of those types** sharing an email or phone, with the same name, or a
//!   similar one (typos, "Jon"/"John", "Acme Ltd"/"Acme Limited"), so an existing contact or a
//!   lead for the same person is used instead of a second record;
//! - **mail senders** not in the CRM yet whose emails came in to a connected mailbox;
//! - **live chat visitors** who left that email.
//!
//! Creating the record with `adopt_history` attaches those earlier emails and chats to it.
use std::collections::HashMap;

use axum::{
    extract::{Path, Query, State},
    routing::get,
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use super::{access, items::normalize, COMPANY, CONTACT, LEAD};
use crate::{auth::AuthUser, error::AppResult, routes::clean, state::AppState};

pub fn router() -> Router<AppState> {
    Router::new().route("/crm/{type}/match", get(matches))
}

#[derive(Deserialize)]
struct Q {
    emails: Option<String>,
    phones: Option<String>,
    /// A person's name (contact or lead).
    name: Option<String>,
    /// A business name (company, or a lead's company).
    company: Option<String>,
    exclude: Option<Uuid>,
}

/// How alike two names must be (trigram similarity) to be shown.
const SIMILAR: f32 = 0.55;

/// A business name without its legal form and punctuation: "ACME Ltd." → "acme".
pub fn company_key(s: &str) -> String {
    const FORMS: &[&str] = &["ltd", "limited", "llc", "inc", "incorporated", "corp", "corporation", "co", "company", "gmbh", "ag", "plc", "sa", "sas", "sarl", "srl", "bv", "nv", "oy", "ab", "as", "pvt", "pty", "llp", "lp", "kg", "spa"];
    let words: Vec<String> = s
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(String::from)
        .collect();
    let mut end = words.len();
    while end > 1 && FORMS.contains(&words[end - 1].as_str()) {
        end -= 1;
    }
    words[..end].join(" ")
}

const COMPANY_KEY_SQL: &str = "trim(regexp_replace(regexp_replace(lower(COALESCE(NULLIF(i.company_name, ''), i.title)), '[^[:alnum:]]+', ' ', 'g'),
    '( (ltd|limited|llc|inc|incorporated|corp|corporation|co|company|gmbh|ag|plc|sa|sas|sarl|srl|bv|nv|oy|ab|as|pvt|pty|llp|lp|kg|spa))+ *$', ''))";

/// A person's name as typed in forms: first, middle and last name (else the record's title).
const PERSON_SQL: &str = "lower(COALESCE(NULLIF(trim(concat_ws(' ', NULLIF(i.first_name, ''), NULLIF(i.middle_name, ''), NULLIF(i.last_name, ''))), ''), i.title))";

#[derive(sqlx::FromRow)]
struct Found {
    id: Uuid,
    entity_type: i16,
    code: String,
    type_name: String,
    title: String,
    assigned_to: Option<Uuid>,
    reasons: Vec<String>,
    score: f32,
}

async fn matches(State(state): State<AppState>, auth: AuthUser, Path(code): Path<String>, Query(q): Query<Q>) -> AppResult<Json<Value>> {
    let t = super::entity_type(&state, &code).await?;
    let split = |s: &Option<String>, kind: &str| -> Vec<String> { s.as_deref().unwrap_or("").split(',').map(|v| normalize(kind, v)).filter(|v| v.len() >= 3).collect() };
    let emails = split(&q.emails, "email");
    let phones = split(&q.phones, "phone");
    let name = clean(q.name.clone()).map(|n| n.to_lowercase()).filter(|n| n.chars().count() >= 4);
    let company = clean(q.company.clone()).map(|c| company_key(&c)).filter(|c| c.chars().count() >= 3);
    // People are matched against people, businesses against businesses (a lead is both).
    let person_types: Vec<i16> = vec![CONTACT, LEAD];
    let business_types: Vec<i16> = vec![COMPANY, LEAD];
    let found: Vec<Found> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT i.id, i.entity_type, t.code, t.name AS type_name, COALESCE(NULLIF(i.title, ''), NULLIF(i.company_name, ''), '') AS title, i.assigned_to,
                array_agg(DISTINCT d.reason ORDER BY d.reason) AS reasons, MAX(d.score) AS score
         FROM (
             SELECT m.item_id AS id, 'phone' AS reason, 1.0::real AS score FROM crm_multifields m WHERE m.kind = 'phone' AND m.normalized = ANY($1)
             UNION ALL SELECT m.item_id, 'email', 1.0::real FROM crm_multifields m WHERE m.kind = 'email' AND m.normalized = ANY($2)
             UNION ALL SELECT i.id, CASE WHEN {PERSON_SQL} = $3 THEN 'name' ELSE 'similar_name' END, similarity({PERSON_SQL}, $3)
               FROM crm_items i WHERE $3::text IS NOT NULL AND i.entity_type = ANY($4) AND i.deleted_at IS NULL AND {PERSON_SQL} % $3 AND similarity({PERSON_SQL}, $3) >= $7
             UNION ALL SELECT i.id, CASE WHEN {COMPANY_KEY_SQL} = $5 THEN 'company' ELSE 'similar_company' END, similarity({COMPANY_KEY_SQL}, $5)
               FROM crm_items i WHERE $5::text IS NOT NULL AND i.entity_type = ANY($6) AND i.deleted_at IS NULL
                 AND similarity({COMPANY_KEY_SQL}, $5) >= $7
         ) d JOIN crm_items i ON i.id = d.id JOIN crm_entity_types t ON t.id = i.entity_type
         WHERE i.deleted_at IS NULL AND i.entity_type IN (1, 3, 4) AND ($8::uuid IS NULL OR i.id <> $8)
         GROUP BY i.id, t.code, t.name
         LIMIT 100"
    )))
    .bind(&phones)
    .bind(&emails)
    .bind(&name)
    .bind(&person_types)
    .bind(&company)
    .bind(&business_types)
    .bind(SIMILAR)
    .bind(q.exclude)
    .fetch_all(&state.db)
    .await?;
    let mut vis: HashMap<i16, access::Visible> = HashMap::new();
    let mut shown = vec![];
    let mut hidden = 0;
    let mut found = found;
    // Exact matches first, then the most alike; the record's own type first.
    found.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal).then((a.entity_type != t.id).cmp(&(b.entity_type != t.id))));
    for f in found {
        if !vis.contains_key(&f.entity_type) {
            vis.insert(f.entity_type, access::visible(&state, &auth, f.entity_type, "read").await?);
        }
        if vis[&f.entity_type].allows(f.assigned_to) {
            shown.push(json!({
                "id": f.id, "type": f.code, "type_name": f.type_name, "title": f.title, "reasons": f.reasons,
                "same_type": f.entity_type == t.id, "link": format!("/crm/{}/{}", f.code, f.id),
            }));
        } else {
            hidden += 1;
        }
    }
    shown.truncate(10);
    let (senders, chats) = history(&state, &emails).await?;
    Ok(Json(json!({ "records": shown, "hidden_count": hidden, "senders": senders, "chats": chats })))
}

/// Emails and live chats from these addresses that no CRM record has yet.
async fn history(state: &AppState, emails: &[String]) -> AppResult<(Value, Value)> {
    if emails.is_empty() {
        return Ok((json!({ "count": 0 }), json!({ "count": 0 })));
    }
    let (n, last_at, last_subject): (i64, Option<DateTime<Utc>>, Option<String>) = sqlx::query!(r#"SELECT COUNT(*) AS "c0!", MAX(e.sent_at) AS "c1?", (array_agg(e.subject ORDER BY e.sent_at DESC))[1] AS "c2?" FROM crm_emails e
         WHERE e.direction = 'in' AND lower(e.from_addr) = ANY($1) AND NOT EXISTS (SELECT 1 FROM crm_email_links l WHERE l.email_id = e.id)"#, emails).map(|r| (r.c0, r.c1, r.c2))
    .fetch_one(&state.db)
    .await?;
    let (c, chat_at): (i64, Option<DateTime<Utc>>) = sqlx::query!(r#"SELECT COUNT(*) AS "c0!", MAX(started_at) AS "c1?" FROM chat_sessions WHERE item_id IS NULL AND lower(visitor_email) = ANY($1)"#, emails).map(|r| (r.c0, r.c1))
        .fetch_one(&state.db)
        .await?;
    Ok((json!({ "count": n, "last_at": last_at, "last_subject": last_subject }), json!({ "count": c, "last_at": chat_at })))
}

/// Attaches the earlier emails (to or from the record's addresses, not linked to any record)
/// and live chats (by the visitor's email, not linked) to a new record. Returns (emails, chats).
pub async fn adopt_history(state: &AppState, item: Uuid) -> AppResult<(u64, u64)> {
    let emails: Vec<String> = sqlx::query_scalar!("SELECT normalized FROM crm_multifields WHERE item_id = $1 AND kind = 'email'", item).fetch_all(&state.db).await?;
    if emails.is_empty() {
        return Ok((0, 0));
    }
    let e = sqlx::query!("INSERT INTO crm_email_links (email_id, item_id)
         SELECT e.id, $2 FROM crm_emails e
         WHERE NOT EXISTS (SELECT 1 FROM crm_email_links l WHERE l.email_id = e.id)
           AND (lower(e.from_addr) = ANY($1)
                OR EXISTS (SELECT 1 FROM jsonb_array_elements(e.to_addrs || e.cc_addrs) x WHERE lower(COALESCE(x->>'email', x #>> '{}')) = ANY($1)))
         ON CONFLICT DO NOTHING", &emails, item)
    .execute(&state.db)
    .await?
    .rows_affected();
    let c = sqlx::query!("UPDATE chat_sessions SET item_id = $2 WHERE item_id IS NULL AND lower(visitor_email) = ANY($1)", &emails, item)
        .execute(&state.db)
        .await?
        .rows_affected();
    if e + c > 0 {
        sqlx::query!("UPDATE crm_items SET last_activity_at = GREATEST(last_activity_at, (SELECT MAX(sent_at) FROM crm_emails e JOIN crm_email_links l ON l.email_id = e.id WHERE l.item_id = $1)) WHERE id = $1", item)
            .execute(&state.db)
            .await?;
    }
    Ok((e, c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn business_names() {
        assert_eq!(company_key("ACME Ltd."), "acme");
        assert_eq!(company_key("Acme Limited"), "acme");
        assert_eq!(company_key("Smith & Co, LLC"), "smith");
        assert_eq!(company_key("Ltd"), "ltd");
        assert_eq!(company_key("Green Garden Services GmbH"), "green garden services");
    }
}
