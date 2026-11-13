//! Tickets and their conversation.
//!
//! The SLA clock: when the customer writes and nothing is pending, the reply is due by the
//! agreement's response time from that moment. Support's reply (not a note) stops the
//! clock and the ticket waits for the customer. Holding a ticket stops the clock too;
//! releasing it starts it again from then. Closing ends it.

use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::FromRow;
use uuid::Uuid;

use super::{default_of, entry_name, entry_of, is_worker, mail, sla, worker};
use crate::{
    auth::{permissions as perm, tokens::random_token, AuthUser},
    crypto,
    error::{AppError, AppResult},
    notify::{notify, NewNotification},
    routes::{clean, required, Page, PageParams},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/support/tickets", get(list).post(create_ticket))
        .route("/support/tickets/{id}", get(get_ticket).patch(update).delete(remove))
        .route("/support/tickets/{id}/messages", post(post_message))
        .route("/support/tickets/{id}/close", post(close))
        .route("/support/tickets/{id}/reopen", post(reopen))
        .route("/support/tickets/{id}/rate", post(rate))
}

#[derive(Serialize, FromRow, Clone)]
pub struct TicketRow {
    pub id: Uuid,
    pub number: i64,
    pub title: String,
    pub channel: String,
    pub requester_id: Option<Uuid>,
    pub requester_name: Option<String>,
    pub requester_email: Option<String>,
    pub contact_id: Option<Uuid>,
    pub category_id: Option<Uuid>,
    pub criticality_id: Option<Uuid>,
    pub status_id: Option<Uuid>,
    pub difficulty_id: Option<Uuid>,
    pub mark_id: Option<Uuid>,
    pub sla_id: Option<Uuid>,
    pub responsible_id: Option<Uuid>,
    pub responsible_name: Option<String>,
    pub waiting_for: String,
    pub waiting_since: Option<DateTime<Utc>>,
    pub deadline_at: Option<DateTime<Utc>>,
    pub is_overdue: bool,
    pub overdue_count: i32,
    pub on_hold: bool,
    pub messages: i32,
    pub last_message_at: DateTime<Utc>,
    pub first_response_at: Option<DateTime<Utc>>,
    pub minutes_spent: i32,
    pub is_spam: bool,
    pub closed_at: Option<DateTime<Utc>>,
    pub auto_closed: bool,
    pub created_at: DateTime<Utc>,
}

pub const TICKET_SELECT: &str = "SELECT t.id, t.number, t.title, t.channel, t.requester_id,
    COALESCE(ru.first_name || ' ' || ru.last_name, t.requester_name, t.requester_email) AS requester_name,
    COALESCE(ru.email, t.requester_email) AS requester_email,
    t.contact_id, t.category_id, t.criticality_id, t.status_id, t.difficulty_id, t.mark_id, t.sla_id,
    t.responsible_id, CASE WHEN re.id IS NULL THEN NULL ELSE re.first_name || ' ' || re.last_name END AS responsible_name,
    t.waiting_for, t.waiting_since, t.deadline_at, t.is_overdue, t.overdue_count, t.on_hold, t.messages, t.last_message_at,
    t.first_response_at, t.minutes_spent, t.is_spam, t.closed_at, t.auto_closed, t.created_at
    FROM support_tickets t LEFT JOIN users ru ON ru.id = t.requester_id LEFT JOIN users re ON re.id = t.responsible_id";

pub async fn load(state: &AppState, id: Uuid) -> AppResult<TicketRow> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{TICKET_SELECT} WHERE t.id = $1")))
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound("ticket"))
}

/// The ticket, if this person may see it: support, its responsible person, its requester.
async fn visible(state: &AppState, auth: &AuthUser, id: Uuid) -> AppResult<TicketRow> {
    let t = load(state, id).await?;
    if worker(auth) || t.requester_id == Some(auth.id) || t.responsible_id == Some(auth.id) {
        Ok(t)
    } else {
        Err(AppError::NotFound("ticket"))
    }
}

/// Answers as support: works on the help desk and is not the one asking.
fn as_support(auth: &AuthUser, t: &TicketRow) -> bool {
    (worker(auth) || t.responsible_id == Some(auth.id)) && t.requester_id != Some(auth.id)
}

// ---------------------------------------------------------------------------
// Making tickets (used by the app, the public page and the email inbox)
// ---------------------------------------------------------------------------

pub struct NewTicket {
    pub title: String,
    pub body: String,
    pub channel: &'static str,
    pub requester_id: Option<Uuid>,
    pub requester_email: Option<String>,
    pub requester_name: Option<String>,
    pub category_id: Option<Uuid>,
    pub criticality_id: Option<Uuid>,
    pub file_ids: Vec<Uuid>,
    pub created_by: Option<Uuid>,
    pub email_message_id: Option<String>,
}

pub struct Created {
    pub id: Uuid,
    pub number: i64,
    /// The customer's secret link token (customers outside the company only).
    pub token: Option<String>,
}

pub async fn create(state: &AppState, n: NewTicket) -> AppResult<Created> {
    let title = required("title", &n.title, 300)?;
    let body = required("message", &n.body, 100_000)?;
    let category = match entry_of(state, n.category_id, "category").await? {
        Some(c) => Some(c),
        None => default_of(state, "category").await?,
    };
    let criticality = match entry_of(state, n.criticality_id, "criticality").await? {
        Some(c) => Some(c),
        None => default_of(state, "criticality").await?,
    };
    let status = default_of(state, "status").await?;
    let staff = n.requester_id.is_some();
    let sla_id = sla::choose(state, category, criticality, staff).await?;
    let sla_responsible: Option<Uuid> = match sla_id {
        Some(s) => sqlx::query_scalar!("SELECT responsible_id FROM support_slas WHERE id = $1", s).fetch_optional(&state.db).await?.flatten(),
        None => None,
    };
    let category_responsible: Option<Uuid> = match category {
        Some(c) => sqlx::query_scalar!("SELECT responsible_id FROM support_dictionary WHERE id = $1", c).fetch_optional(&state.db).await?.flatten(),
        None => None,
    };
    let mut responsible = None;
    for r in [category_responsible, sla_responsible].into_iter().flatten() {
        if Some(r) != n.requester_id && is_worker(state, r).await? {
            responsible = Some(r);
            break;
        }
    }
    let contact: Option<Uuid> = match &n.requester_email {
        Some(e) if !staff => sqlx::query_scalar!("SELECT i.id FROM crm_multifields m JOIN crm_items i ON i.id = m.item_id
             WHERE m.kind = 'email' AND m.normalized = $1 AND i.entity_type = $2 AND i.deleted_at IS NULL ORDER BY i.created_at LIMIT 1", e.to_lowercase(), crate::crm::CONTACT)
        .fetch_optional(&state.db)
        .await?,
        _ => None,
    };
    let token = (!staff).then(|| random_token(24));
    let now = Utc::now();
    let (deadline, notice) = sla::due(state, sla_id, now).await?;
    let mut tx = state.db.begin().await?;
    let (id, number): (Uuid, i64) = sqlx::query!(r#"INSERT INTO support_tickets (title, channel, requester_id, requester_email, requester_name, contact_id, category_id, criticality_id, status_id,
             sla_id, responsible_id, access_hash, access_secret, waiting_for, waiting_since, deadline_at, notice_at, messages, last_message_at, created_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, 'support', $14, $15, $16, 1, $14, $17) RETURNING id, number"#, &title, n.channel, n.requester_id, n.requester_email.as_ref().filter(|_| !staff).map(|e| e.to_lowercase()), n.requester_name.as_ref().filter(|_| !staff), contact, category, criticality, status, sla_id, responsible, token.as_deref().map(super::hash), token.as_deref().map(|t| crypto::seal(&state.config.data_key, t)), now, deadline, notice, n.created_by).map(|r| (r.id, r.number))
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query!("INSERT INTO support_messages (ticket_id, kind, by_support, author_id, author_email, author_name, body, file_ids, email_message_id, created_at)
         VALUES ($1, 'message', FALSE, $2, $3, $4, $5, $6, $7, $8)", id, n.requester_id, n.requester_email.as_ref().filter(|_| !staff), n.requester_name.as_ref().filter(|_| !staff), &body, &n.file_ids, n.email_message_id.as_deref(), now)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    if let Some(r) = responsible {
        notify(state, &[r], NewNotification {
            kind: "support.assigned",
            title: format!("New ticket #{number}: {title}"),
            body: Some(body.chars().take(200).collect()),
            link: Some(format!("/support/tickets/{id}")),
            actor: n.requester_id.or(n.created_by),
        })
        .await;
    }
    if !staff {
        mail::tell_customer(state, id, mail::Notice::Received).await?;
    }
    Ok(Created { id, number, token })
}

/// Who wrote a message.
pub enum Author {
    /// Someone in the company (support, or an employee asking).
    User(Uuid),
    /// A customer, by the public page or email.
    Customer { email: String, name: Option<String> },
}

pub struct NewMessage {
    pub body: String,
    pub note: bool,
    pub file_ids: Vec<Uuid>,
    pub minutes_spent: i32,
    pub email_message_id: Option<String>,
}

/// Adds a message and moves the ticket along: a customer message (re)starts the SLA clock
/// (and reopens a closed ticket when allowed); support's reply stops it.
pub async fn add_message(state: &AppState, t: &TicketRow, author: Author, by_support: bool, m: NewMessage) -> AppResult<Uuid> {
    let body = required("message", &m.body, 100_000)?;
    if !(0..=24 * 60).contains(&m.minutes_spent) {
        return Err(AppError::bad("time spent is 0 to 1440 minutes"));
    }
    if m.note && !by_support {
        return Err(AppError::bad("only support writes notes"));
    }
    let (author_id, author_email, author_name) = match &author {
        Author::User(u) => (Some(*u), None, None),
        Author::Customer { email, name } => (None, Some(email.clone()), name.clone()),
    };
    let now = Utc::now();
    let mut reopened = false;
    if !m.note && !by_support && t.closed_at.is_some() {
        let allow: bool = sqlx::query_scalar!("SELECT allow_reopen FROM support_settings WHERE id").fetch_one(&state.db).await?;
        if !allow && t.requester_id.is_none() {
            return Err(AppError::Conflict("this request is closed; please send a new one".into()));
        }
        reopened = true;
    }
    let late = by_support && !m.note && t.deadline_at.is_some_and(|d| d < now);
    let id: Uuid = sqlx::query_scalar!("INSERT INTO support_messages (ticket_id, kind, by_support, author_id, author_email, author_name, body, file_ids, minutes_spent, is_overdue,
             email_message_id, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12) RETURNING id", t.id, if m.note { "note" } else { "message" }, by_support, author_id, author_email, author_name, &body, &m.file_ids, m.minutes_spent, late, m.email_message_id.as_deref(), now)
    .fetch_one(&state.db)
    .await?;
    sqlx::query!("UPDATE support_tickets SET minutes_spent = minutes_spent + $2, updated_at = now() WHERE id = $1", t.id, m.minutes_spent).execute(&state.db).await?;
    if m.note {
        return Ok(id);
    }
    sqlx::query!("UPDATE support_tickets SET messages = messages + 1, last_message_at = $2 WHERE id = $1", t.id, now).execute(&state.db).await?;
    if by_support {
        sqlx::query!("UPDATE support_tickets SET waiting_for = 'customer', waiting_since = NULL, deadline_at = NULL, notice_at = NULL, notice_sent = FALSE,
                 is_overdue = FALSE, first_response_at = COALESCE(first_response_at, $2)
             WHERE id = $1", t.id, now)
        .execute(&state.db)
        .await?;
        tell_requester(state, t, Some(id), author_id).await?;
    } else {
        if reopened {
            sqlx::query!("UPDATE support_tickets SET closed_at = NULL, closed_by = NULL, auto_closed = FALSE, mark_id = NULL WHERE id = $1", t.id).execute(&state.db).await?;
            log(state, t.id, author_id, "Reopened by a new message".into()).await?;
        }
        // The clock starts with the first unanswered message, not each one after it.
        if reopened || t.waiting_for == "customer" {
            start_clock(state, t.id, t.sla_id, now).await?;
        }
        if let Some(r) = t.responsible_id {
            notify(state, &[r], NewNotification {
                kind: "support.message",
                title: format!("{} on #{}: {}", if reopened { "Reopened" } else { "New message" }, t.number, t.title),
                body: Some(body.chars().take(200).collect()),
                link: Some(format!("/support/tickets/{}", t.id)),
                actor: author_id,
            })
            .await;
        }
    }
    Ok(id)
}

async fn start_clock(state: &AppState, ticket: Uuid, sla_id: Option<Uuid>, since: DateTime<Utc>) -> AppResult<()> {
    let hold: bool = sqlx::query_scalar!("SELECT on_hold FROM support_tickets WHERE id = $1", ticket).fetch_one(&state.db).await?;
    let (deadline, notice) = if hold { (None, None) } else { sla::due(state, sla_id, since).await? };
    sqlx::query!("UPDATE support_tickets SET waiting_for = 'support', waiting_since = $2, deadline_at = $3, notice_at = $4, notice_sent = FALSE, is_overdue = FALSE
         WHERE id = $1", ticket, since, deadline, notice)
    .execute(&state.db)
    .await?;
    Ok(())
}

/// Something changed: kept in the ticket's history (support sees it).
pub async fn log(state: &AppState, ticket: Uuid, by: Option<Uuid>, text: String) -> AppResult<()> {
    sqlx::query!("INSERT INTO support_messages (ticket_id, kind, by_support, author_id, body) VALUES ($1, 'log', TRUE, $2, $3)", ticket, by, text)
        .execute(&state.db)
        .await?;
    Ok(())
}

/// Support answered: the employee who asked gets a notification, a customer an email.
async fn tell_requester(state: &AppState, t: &TicketRow, message: Option<Uuid>, actor: Option<Uuid>) -> AppResult<()> {
    match t.requester_id {
        Some(r) => {
            notify(state, &[r], NewNotification {
                kind: "support.reply",
                title: format!("Answer to your request #{}: {}", t.number, t.title),
                body: None,
                link: Some(format!("/support/tickets/{}", t.id)),
                actor,
            })
            .await
        }
        None => {
            if let Some(m) = message {
                mail::tell_customer(state, t.id, mail::Notice::Reply(m)).await?;
            }
        }
    }
    Ok(())
}

async fn closed_notice(state: &AppState, t: &TicketRow, actor: Option<Uuid>, auto: bool) -> AppResult<()> {
    match t.requester_id {
        Some(r) => {
            notify(state, &[r], NewNotification {
                kind: "support.closed",
                title: format!("Your request #{} was closed: {}", t.number, t.title),
                body: Some("Rate the answer, or write again if it is not solved.".into()),
                link: Some(format!("/support/tickets/{}", t.id)),
                actor,
            })
            .await
        }
        None => mail::tell_customer(state, t.id, mail::Notice::Closed { auto }).await?,
    }
    Ok(())
}

/// The SLA job closed a ticket that waited too long for the customer.
pub async fn after_auto_close(state: &AppState, id: Uuid, days: i32) -> AppResult<()> {
    log(state, id, None, format!("Closed automatically: no answer from the customer in {days} days")).await?;
    let t = load(state, id).await?;
    closed_notice(state, &t, None, true).await
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct ListQuery {
    /// mine | assigned | unassigned | open | overdue | closed | spam | all
    scope: Option<String>,
    status_id: Option<Uuid>,
    category_id: Option<Uuid>,
    criticality_id: Option<Uuid>,
    responsible_id: Option<Uuid>,
    q: Option<String>,
    #[serde(flatten)]
    page: PageParams,
}

async fn list(State(state): State<AppState>, auth: AuthUser, Query(q): Query<ListQuery>) -> AppResult<Json<Page<TicketRow>>> {
    let w = worker(&auth);
    let scope = q.scope.as_deref().unwrap_or(if w { "open" } else { "mine" });
    let mut cond = vec![if w { "TRUE".to_string() } else { "(t.requester_id = $1 OR t.responsible_id = $1)".to_string() }];
    cond.push(
        match scope {
            "mine" => "t.requester_id = $1",
            "assigned" => "t.responsible_id = $1 AND t.closed_at IS NULL AND NOT t.is_spam",
            "unassigned" => "t.responsible_id IS NULL AND t.closed_at IS NULL AND NOT t.is_spam",
            "open" => "t.closed_at IS NULL AND NOT t.is_spam",
            "overdue" => "t.is_overdue AND t.closed_at IS NULL AND NOT t.is_spam",
            "closed" => "t.closed_at IS NOT NULL AND NOT t.is_spam",
            "spam" => "t.is_spam",
            "all" => "NOT t.is_spam",
            _ => return Err(AppError::bad("unknown list")),
        }
        .to_string(),
    );
    if q.status_id.is_some() {
        cond.push("t.status_id = $2".into());
    }
    if q.category_id.is_some() {
        cond.push("t.category_id = $3".into());
    }
    if q.criticality_id.is_some() {
        cond.push("t.criticality_id = $4".into());
    }
    if q.responsible_id.is_some() {
        cond.push("t.responsible_id = $5".into());
    }
    let search = clean(q.q.clone());
    if search.is_some() {
        cond.push("(t.title ILIKE '%' || $6 || '%' OR t.number::text = ltrim($6, '#') OR t.requester_email ILIKE '%' || $6 || '%')".into());
    }
    let where_ = cond.join(" AND ");
    let order = if scope == "closed" || scope == "mine" || scope == "all" || scope == "spam" {
        "t.last_message_at DESC"
    } else {
        "t.deadline_at ASC NULLS LAST, t.last_message_at DESC"
    };
    let items: Vec<TicketRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!("{TICKET_SELECT} WHERE {where_} ORDER BY {order} LIMIT $7 OFFSET $8")))
        .bind(auth.id)
        .bind(q.status_id)
        .bind(q.category_id)
        .bind(q.criticality_id)
        .bind(q.responsible_id)
        .bind(&search)
        .bind(q.page.limit())
        .bind(q.page.offset())
        .fetch_all(&state.db)
        .await?;
    let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT count(*) FROM support_tickets t WHERE {where_} AND $7::int8 IS NOT NULL AND $8::int8 IS NOT NULL"
    )))
    .bind(auth.id)
    .bind(q.status_id)
    .bind(q.category_id)
    .bind(q.criticality_id)
    .bind(q.responsible_id)
    .bind(&search)
    .bind(q.page.limit())
    .bind(q.page.offset())
    .fetch_one(&state.db)
    .await?;
    Ok(Json(Page { items, total, limit: q.page.limit(), offset: q.page.offset() }))
}

#[derive(Deserialize)]
struct CreateInput {
    title: String,
    body: String,
    category_id: Option<Uuid>,
    criticality_id: Option<Uuid>,
    #[serde(default)]
    file_ids: Vec<Uuid>,
    /// Support taking a request for a customer (by phone, in person).
    customer_email: Option<String>,
    customer_name: Option<String>,
    /// Support taking a request for a colleague.
    requester_id: Option<Uuid>,
}

async fn own_files(state: &AppState, auth: &AuthUser, ids: &[Uuid]) -> AppResult<Vec<Uuid>> {
    if ids.len() > 20 {
        return Err(AppError::bad("at most 20 files"));
    }
    for f in ids {
        let owner: Option<Option<Uuid>> = sqlx::query_scalar!(r#"SELECT owner_id AS "owner_id?" FROM files WHERE id = $1"#, f).fetch_optional(&state.db).await?;
        if owner.flatten() != Some(auth.id) {
            return Err(AppError::NotFound("file"));
        }
    }
    Ok(ids.to_vec())
}

async fn create_ticket(State(state): State<AppState>, auth: AuthUser, Json(b): Json<CreateInput>) -> AppResult<Json<Value>> {
    let files = own_files(&state, &auth, &b.file_ids).await?;
    let customer = clean(b.customer_email);
    if (customer.is_some() || b.requester_id.is_some()) && !worker(&auth) {
        return Err(AppError::Forbidden);
    }
    if customer.is_some() && b.requester_id.is_some() {
        return Err(AppError::bad("a request is for a colleague or for a customer, not both"));
    }
    let requester = match (&customer, b.requester_id) {
        (Some(_), _) => None,
        (None, Some(r)) => {
            let ok: bool = sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM users WHERE id = $1 AND is_active AND NOT is_system) AS "v!""#, r).fetch_one(&state.db).await?;
            if !ok {
                return Err(AppError::bad("unknown colleague"));
            }
            Some(r)
        }
        (None, None) => Some(auth.id),
    };
    let email = customer.map(|e| crate::routes::normalize_email(&e)).transpose()?;
    let c = create(&state, NewTicket {
        title: b.title,
        body: b.body,
        channel: "staff",
        requester_id: requester,
        requester_email: email,
        requester_name: clean(b.customer_name),
        category_id: b.category_id,
        criticality_id: b.criticality_id,
        file_ids: files,
        created_by: Some(auth.id),
        email_message_id: None,
    })
    .await?;
    get_ticket(State(state), auth, Path(c.id)).await
}

async fn ticket_json(state: &AppState, auth: &AuthUser, t: &TicketRow) -> AppResult<Value> {
    let support = as_support(auth, t) || worker(auth);
    let rows: Vec<(Uuid, String, bool, Option<Uuid>, Option<String>, Option<String>, Option<Uuid>, String, Vec<Uuid>, i32, bool, Option<String>, DateTime<Utc>)> =
        sqlx::query!(r#"SELECT m.id, m.kind, m.by_support, m.author_id, COALESCE(u.first_name || ' ' || u.last_name, m.author_name, m.author_email) AS "c4", m.author_email,
                    u.avatar_file_id, m.body, m.file_ids, m.minutes_spent, m.is_overdue, m.email_status, m.created_at
             FROM support_messages m LEFT JOIN users u ON u.id = m.author_id
             WHERE m.ticket_id = $1 AND ($2 OR m.kind = 'message') ORDER BY m.created_at, m.id"#, t.id, support).map(|r| (r.id, r.kind, r.by_support, r.author_id, r.c4, r.author_email, r.avatar_file_id, r.body, r.file_ids, r.minutes_spent, r.is_overdue, r.email_status, r.created_at))
        .fetch_all(&state.db)
        .await?;
    let mut messages = Vec::new();
    for (id, kind, by_support, author_id, name, email, avatar, body, file_ids, minutes, late, email_status, at) in rows {
        let files: Vec<(Uuid, String, i64)> = sqlx::query!(r#"SELECT id, original_name, size_bytes FROM files WHERE id = ANY($1)"#, &file_ids).map(|r| (r.id, r.original_name, r.size_bytes)).fetch_all(&state.db).await?;
        messages.push(json!({
            "id": id, "kind": kind, "by_support": by_support, "author_id": author_id, "author_name": name.unwrap_or_else(|| "System".into()),
            "author_email": if support { email } else { None }, "author_avatar_file_id": avatar, "body": body,
            "files": files.into_iter().map(|(id, name, size)| json!({ "id": id, "name": name, "size": size })).collect::<Vec<_>>(),
            "minutes_spent": if support { minutes } else { 0 }, "is_overdue": support && late, "email_status": if support { email_status } else { None }, "created_at": at,
        }));
    }
    let mut v = serde_json::to_value(t).unwrap();
    v["messages"] = json!(messages);
    v["as_support"] = json!(as_support(auth, t));
    v["can_rate"] = json!(t.closed_at.is_some() && t.requester_id == Some(auth.id));
    if let Some(s) = t.sla_id {
        let name: Option<String> = sqlx::query_scalar!("SELECT name FROM support_slas WHERE id = $1", s).fetch_optional(&state.db).await?;
        v["sla_name"] = json!(name);
    }
    if !support {
        for k in ["minutes_spent", "difficulty_id", "sla_id", "overdue_count", "is_spam"] {
            v.as_object_mut().unwrap().remove(k);
        }
    }
    Ok(v)
}

async fn get_ticket(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let t = visible(&state, &auth, id).await?;
    Ok(Json(ticket_json(&state, &auth, &t).await?))
}

#[derive(Deserialize)]
struct MessageInput {
    body: String,
    #[serde(default)]
    note: bool,
    #[serde(default)]
    file_ids: Vec<Uuid>,
    #[serde(default)]
    minutes_spent: i32,
    /// Support: set this status with the reply.
    status_id: Option<Uuid>,
    /// Support: close the ticket with the reply.
    #[serde(default)]
    close: bool,
}

async fn post_message(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<MessageInput>) -> AppResult<Json<Value>> {
    let t = visible(&state, &auth, id).await?;
    let support = as_support(&auth, &t);
    if !support && (b.status_id.is_some() || b.close || b.minutes_spent != 0) {
        return Err(AppError::Forbidden);
    }
    if t.is_spam {
        return Err(AppError::bad("this ticket is marked as spam"));
    }
    let files = own_files(&state, &auth, &b.file_ids).await?;
    add_message(&state, &t, Author::User(auth.id), support, NewMessage { body: b.body, note: b.note, file_ids: files, minutes_spent: b.minutes_spent, email_message_id: None }).await?;
    if let Some(s) = b.status_id {
        change(&state, &auth, id, Changes { status_id: Some(Some(s)), ..Default::default() }).await?;
    }
    if b.close {
        do_close(&state, &auth, id).await?;
    }
    get_ticket(State(state), auth, Path(id)).await
}

#[derive(Deserialize, Default)]
struct Changes {
    title: Option<String>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    category_id: Option<Option<Uuid>>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    criticality_id: Option<Option<Uuid>>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    status_id: Option<Option<Uuid>>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    difficulty_id: Option<Option<Uuid>>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    responsible_id: Option<Option<Uuid>>,
    sla_id: Option<Uuid>,
    on_hold: Option<bool>,
    is_spam: Option<bool>,
}

async fn update(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<Changes>) -> AppResult<Json<Value>> {
    change(&state, &auth, id, b).await?;
    get_ticket(State(state), auth, Path(id)).await
}

async fn user_name(state: &AppState, id: Option<Uuid>) -> AppResult<String> {
    Ok(match id {
        Some(u) => sqlx::query_scalar!(r#"SELECT first_name || ' ' || last_name AS "v!" FROM users WHERE id = $1"#, u).fetch_optional(&state.db).await?.unwrap_or_default(),
        None => "nobody".into(),
    })
}

async fn change(state: &AppState, auth: &AuthUser, id: Uuid, b: Changes) -> AppResult<()> {
    let t = visible(state, auth, id).await?;
    if !as_support(auth, &t) && !worker(auth) {
        return Err(AppError::Forbidden);
    }
    let mut history = Vec::new();
    let mut reschedule = false;
    let dict = |field: &'static str, kind: &'static str, cur: Option<Uuid>, new: Option<Option<Uuid>>| async move {
        match new {
            Some(n) if n != cur => {
                entry_of(state, n, kind).await?;
                let (a, b) = (entry_name(state, cur).await?, entry_name(state, n).await?);
                AppResult::Ok(Some((field, n, format!("{}: {} → {}", capitalize(field), or_none(a), or_none(b)))))
            }
            _ => Ok(None),
        }
    };
    let mut sets: Vec<(&'static str, Option<Uuid>)> = Vec::new();
    for (field, kind, cur, new) in [
        ("category", "category", t.category_id, b.category_id),
        ("criticality", "criticality", t.criticality_id, b.criticality_id),
        ("status", "status", t.status_id, b.status_id),
        ("difficulty", "difficulty", t.difficulty_id, b.difficulty_id),
    ] {
        if let Some((f, v, text)) = dict(field, kind, cur, new).await? {
            sets.push((f, v));
            history.push(text);
            reschedule |= f == "category" || f == "criticality";
        }
    }
    let mut tx = state.db.begin().await?;
    for (f, v) in &sets {
        sqlx::query(sqlx::AssertSqlSafe(format!("UPDATE support_tickets SET {f}_id = $2, updated_at = now() WHERE id = $1"))).bind(id).bind(v).execute(&mut *tx).await?;
    }
    if let Some(title) = b.title {
        let title = required("title", &title, 300)?;
        if title != t.title {
            sqlx::query!("UPDATE support_tickets SET title = $2 WHERE id = $1", id, &title).execute(&mut *tx).await?;
            history.push(format!("Title: {} → {}", t.title, title));
        }
    }
    let mut assigned = None;
    if let Some(r) = b.responsible_id
        && r != t.responsible_id {
            if let Some(u) = r
                && !is_worker(state, u).await? {
                    return Err(AppError::bad("tickets go to people who work on the help desk"));
                }
            sqlx::query!("UPDATE support_tickets SET responsible_id = $2 WHERE id = $1", id, r).execute(&mut *tx).await?;
            history.push(format!("Responsible: {} → {}", user_name(state, t.responsible_id).await?, user_name(state, r).await?));
            assigned = r;
        }
    let mut new_sla = None;
    if let Some(s) = b.sla_id
        && Some(s) != t.sla_id {
            let name: String = sqlx::query_scalar!("SELECT name FROM support_slas WHERE id = $1", s).fetch_optional(&state.db).await?.ok_or_else(|| AppError::bad("unknown SLA"))?;
            history.push(format!("SLA: {name}"));
            new_sla = Some(s);
        }
    if new_sla.is_none() && reschedule {
        let cat = sets.iter().find(|(f, _)| *f == "category").map(|(_, v)| *v).unwrap_or(t.category_id);
        let crit = sets.iter().find(|(f, _)| *f == "criticality").map(|(_, v)| *v).unwrap_or(t.criticality_id);
        let chosen = sla::choose(state, cat, crit, t.requester_id.is_some()).await?;
        if chosen != t.sla_id {
            if let Some(s) = chosen {
                let name: String = sqlx::query_scalar!("SELECT name FROM support_slas WHERE id = $1", s).fetch_one(&state.db).await?;
                history.push(format!("SLA: {name}"));
            }
            new_sla = chosen;
        }
    }
    if let Some(s) = new_sla {
        sqlx::query!("UPDATE support_tickets SET sla_id = $2 WHERE id = $1", id, s).execute(&mut *tx).await?;
    }
    let mut hold_changed = None;
    if let Some(h) = b.on_hold
        && h != t.on_hold {
            sqlx::query!("UPDATE support_tickets SET on_hold = $2 WHERE id = $1", id, h).execute(&mut *tx).await?;
            history.push(if h { "Put on hold".into() } else { "Taken off hold".into() });
            hold_changed = Some(h);
        }
    if let Some(s) = b.is_spam
        && s != t.is_spam {
            sqlx::query!("UPDATE support_tickets SET is_spam = $2, deadline_at = CASE WHEN $2 THEN NULL ELSE deadline_at END, notice_at = CASE WHEN $2 THEN NULL ELSE notice_at END WHERE id = $1", id, s)
                .execute(&mut *tx)
                .await?;
            history.push(if s { "Marked as spam".into() } else { "Not spam".into() });
        }
    tx.commit().await?;
    for h in history {
        log(state, id, Some(auth.id), h).await?;
    }
    // The clock: holding stops it; releasing starts it from now; a new SLA recounts it.
    if t.closed_at.is_none() && t.waiting_for == "support" {
        match hold_changed {
            Some(true) => {
                sqlx::query!("UPDATE support_tickets SET deadline_at = NULL, notice_at = NULL WHERE id = $1", id).execute(&state.db).await?;
            }
            Some(false) => start_clock(state, id, new_sla.or(t.sla_id), Utc::now()).await?,
            None => {
                if new_sla.is_some() && !t.on_hold {
                    start_clock(state, id, new_sla, t.waiting_since.unwrap_or_else(Utc::now)).await?;
                }
            }
        }
    }
    if let Some(r) = assigned {
        notify(state, &[r], NewNotification {
            kind: "support.assigned",
            title: format!("Ticket #{} is yours: {}", t.number, t.title),
            body: None,
            link: Some(format!("/support/tickets/{id}")),
            actor: Some(auth.id),
        })
        .await;
    }
    Ok(())
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

fn or_none(s: String) -> String {
    if s.is_empty() { "none".into() } else { s }
}

async fn do_close(state: &AppState, auth: &AuthUser, id: Uuid) -> AppResult<()> {
    let t = visible(state, auth, id).await?;
    if t.closed_at.is_some() {
        return Ok(());
    }
    sqlx::query!("UPDATE support_tickets SET closed_at = now(), closed_by = $2, auto_closed = FALSE, deadline_at = NULL, notice_at = NULL, updated_at = now() WHERE id = $1", id, auth.id)
        .execute(&state.db)
        .await?;
    log(state, id, Some(auth.id), "Closed".into()).await?;
    if as_support(auth, &t) {
        closed_notice(state, &t, Some(auth.id), false).await?;
    } else if let Some(r) = t.responsible_id {
        notify(state, &[r], NewNotification {
            kind: "support.closed",
            title: format!("#{} was closed by the requester: {}", t.number, t.title),
            body: None,
            link: Some(format!("/support/tickets/{id}")),
            actor: Some(auth.id),
        })
        .await;
    }
    Ok(())
}

async fn close(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    do_close(&state, &auth, id).await?;
    get_ticket(State(state), auth, Path(id)).await
}

/// Reopens a closed ticket; it waits for support again.
pub async fn do_reopen(state: &AppState, t: &TicketRow, by: Option<Uuid>) -> AppResult<()> {
    if t.closed_at.is_none() {
        return Ok(());
    }
    sqlx::query!("UPDATE support_tickets SET closed_at = NULL, closed_by = NULL, auto_closed = FALSE, mark_id = NULL, updated_at = now() WHERE id = $1", t.id)
        .execute(&state.db)
        .await?;
    log(state, t.id, by, "Reopened".into()).await?;
    start_clock(state, t.id, t.sla_id, Utc::now()).await
}

async fn reopen(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let t = visible(&state, &auth, id).await?;
    do_reopen(&state, &t, Some(auth.id)).await?;
    get_ticket(State(state), auth, Path(id)).await
}

#[derive(Deserialize)]
pub struct RateInput {
    pub mark_id: Uuid,
}

/// The requester rates the answer of a closed ticket.
pub async fn do_rate(state: &AppState, t: &TicketRow, mark: Uuid, by: Option<Uuid>) -> AppResult<()> {
    if t.closed_at.is_none() {
        return Err(AppError::bad("rate the answer once the request is closed"));
    }
    entry_of(state, Some(mark), "mark").await?;
    sqlx::query!("UPDATE support_tickets SET mark_id = $2 WHERE id = $1", t.id, mark).execute(&state.db).await?;
    log(state, t.id, by, format!("Rated: {}", entry_name(state, Some(mark)).await?)).await?;
    if let Some(r) = t.responsible_id {
        notify(state, &[r], NewNotification {
            kind: "support.rated",
            title: format!("#{} was rated “{}”", t.number, entry_name(state, Some(mark)).await?),
            body: Some(t.title.clone()),
            link: Some(format!("/support/tickets/{}", t.id)),
            actor: by,
        })
        .await;
    }
    Ok(())
}

async fn rate(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<RateInput>) -> AppResult<Json<Value>> {
    let t = visible(&state, &auth, id).await?;
    if t.requester_id != Some(auth.id) {
        return Err(AppError::Forbidden);
    }
    do_rate(&state, &t, b.mark_id, Some(auth.id)).await?;
    get_ticket(State(state), auth, Path(id)).await
}

async fn remove(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    auth.require(perm::SUPPORT_MANAGE)?;
    let n = sqlx::query!("DELETE FROM support_tickets WHERE id = $1", id).execute(&state.db).await?.rows_affected();
    if n == 0 {
        return Err(AppError::NotFound("ticket"));
    }
    crate::audit::record(&state.db, Some(auth.id), "support.ticket.delete", "support_ticket", Some(id.to_string()), json!({}), None).await;
    Ok(Json(json!({ "ok": true })))
}
