//! HR documents to sign (employment contracts, policies, orders).
//!
//! HR makes a document from a company template, filled in with each employee's details, or
//! from a file (PDF, or Word/OpenDocument turned into PDF), for one employee or many at once
//! (each gets their own). Its steps run in order: approvers approve (or send it back), the
//! company's representative signs, then the employee signs; each is told when it's their
//! turn. People sign inside NERP (signed in), typing their name and drawing their signature;
//! time, address and browser are recorded. When everyone has signed, a signed copy with a
//! certificate page is kept and its fingerprint published (anyone can check a copy at
//! /api/public/verify). HR can invalidate a signed document, giving a reason. Signing raises
//! the `hr.document.signed` event for outside systems.

use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap, HeaderValue},
    response::{IntoResponse, Response},
    routing::{get, patch, post},
    Json, Router,
};
use base64::Engine;
use chrono::{DateTime, NaiveDate, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use super::{hr_manager, time::tz_of};
use crate::{
    auth::{AuthUser, ClientIp},
    crm::esign::{company, sha256_hex, signed_copy, CertSigner},
    error::{AppError, AppResult},
    notify::{notify, NewNotification},
    routes::{
        files::{read_bytes, store_bytes},
        required,
    },
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/hr/doc-templates", get(list_templates).post(create_template))
        .route("/hr/doc-templates/{id}", patch(update_template).delete(delete_template))
        .route("/hr/docs", get(list).post(create))
        .route("/hr/docs/{id}", get(one))
        .route("/hr/docs/{id}/file", get(file))
        .route("/hr/docs/{id}/approve", post(approve))
        .route("/hr/docs/{id}/sign", post(sign))
        .route("/hr/docs/{id}/decline", post(decline))
        .route("/hr/docs/{id}/revoke", post(revoke))
        .route("/public/verify/{sha}", get(verify))
}

fn need_hr(auth: &AuthUser) -> AppResult<()> {
    if hr_manager(auth) { Ok(()) } else { Err(AppError::Forbidden) }
}

// ---------------------------------------------------------------------------
// Templates
// ---------------------------------------------------------------------------

async fn list_templates(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    need_hr(&auth)?;
    let rows: Vec<(Uuid, String, String, DateTime<Utc>)> = sqlx::query_as("SELECT id, name, body, updated_at FROM hr_doc_templates ORDER BY name").fetch_all(&state.db).await?;
    Ok(Json(json!({ "items": rows.into_iter().map(|(id, n, b, u)| json!({ "id": id, "name": n, "body": b, "updated_at": u })).collect::<Vec<_>>() })))
}

#[derive(Deserialize)]
struct TemplateInput {
    name: Option<String>,
    body: Option<String>,
}

async fn create_template(State(state): State<AppState>, auth: AuthUser, Json(b): Json<TemplateInput>) -> AppResult<Json<Value>> {
    need_hr(&auth)?;
    let name = required("name", b.name.as_deref().unwrap_or(""), 200)?;
    let body = required("text", b.body.as_deref().unwrap_or(""), 100_000)?;
    sqlx::query("INSERT INTO hr_doc_templates (name, body, created_by) VALUES ($1, $2, $3)").bind(name).bind(body).bind(auth.id).execute(&state.db).await?;
    list_templates(State(state), auth).await
}

async fn update_template(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<TemplateInput>) -> AppResult<Json<Value>> {
    need_hr(&auth)?;
    let name = b.name.as_deref().map(|n| required("name", n, 200)).transpose()?;
    let body = b.body.as_deref().map(|n| required("text", n, 100_000)).transpose()?;
    sqlx::query("UPDATE hr_doc_templates SET name = COALESCE($2, name), body = COALESCE($3, body), updated_at = now() WHERE id = $1").bind(id).bind(name).bind(body).execute(&state.db).await?;
    list_templates(State(state), auth).await
}

async fn delete_template(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    need_hr(&auth)?;
    sqlx::query("DELETE FROM hr_doc_templates WHERE id = $1").bind(id).execute(&state.db).await?;
    list_templates(State(state), auth).await
}

const PAGE: &str = r##"#set page(paper: "a4", margin: 2.2cm)
#set text(font: "Libertinus Serif", size: 11pt)
#set par(justify: true)
#let d = json("/data.json")
#text(size: 16pt, weight: "bold")[#d.title]
#v(10pt)
#for p in d.paragraphs {
  if p.heading { text(size: 13pt, weight: "bold")[#p.text] } else { p.text }
  parbreak()
}
#v(18pt)
#text(fill: luma(90), size: 9pt)[#d.company · #d.date]
"##;

/// The template's text with an employee's details, as a PDF.
async fn render(state: &AppState, title: &str, body: &str, employee: Uuid) -> AppResult<Vec<u8>> {
    let (first, last, email, position): (String, String, String, Option<String>) =
        sqlx::query_as("SELECT first_name, last_name, email, position FROM users WHERE id = $1").bind(employee).fetch_one(&state.db).await?;
    let dept: Option<String> = sqlx::query_scalar("SELECT d.name FROM user_departments ud JOIN departments d ON d.id = ud.department_id WHERE ud.user_id = $1 ORDER BY d.name LIMIT 1").bind(employee).fetch_optional(&state.db).await?;
    let tz = tz_of(state, employee).await?;
    let date = Utc::now().with_timezone(&tz).format("%-d %B %Y").to_string();
    let company = company(state).await;
    let fill = |t: &str| {
        t.replace("{{name}}", format!("{first} {last}").trim())
            .replace("{{first_name}}", &first)
            .replace("{{last_name}}", &last)
            .replace("{{email}}", &email)
            .replace("{{position}}", position.as_deref().unwrap_or(""))
            .replace("{{department}}", dept.as_deref().unwrap_or(""))
            .replace("{{date}}", &date)
            .replace("{{company}}", &company)
    };
    let paragraphs: Vec<Value> = fill(body)
        .split("\n\n")
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(|p| match p.strip_prefix("# ") {
            Some(h) => json!({ "heading": true, "text": h }),
            None => json!({ "heading": false, "text": p }),
        })
        .collect();
    let data = json!({ "title": fill(title), "paragraphs": paragraphs, "company": company, "date": date });
    tokio::task::spawn_blocking(move || crate::crm::docs::typst_engine::render_with(PAGE, &data, vec![]))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .map_err(|e| AppError::bad(format!("the document could not be made: {e}")))
}

// ---------------------------------------------------------------------------
// Documents
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct NewDocs {
    title: String,
    template_id: Option<Uuid>,
    /// Or a file uploaded first (PDF, Word, OpenDocument).
    file_id: Option<Uuid>,
    employee_ids: Vec<Uuid>,
    #[serde(default)]
    approver_ids: Vec<Uuid>,
    /// Signs for the company (before the employee).
    company_signer_id: Option<Uuid>,
    due_on: Option<NaiveDate>,
}

/// Makes one document per employee and starts its steps.
async fn create(State(state): State<AppState>, auth: AuthUser, Json(b): Json<NewDocs>) -> AppResult<Json<Value>> {
    need_hr(&auth)?;
    let title = required("title", &b.title, 300)?;
    if b.employee_ids.is_empty() || b.employee_ids.len() > 500 {
        return Err(AppError::bad("choose 1 to 500 employees"));
    }
    if b.due_on.is_some_and(|d| d < Utc::now().date_naive()) {
        return Err(AppError::bad("the due date has passed"));
    }
    let mut people: Vec<Uuid> = b.employee_ids.clone();
    people.extend(&b.approver_ids);
    people.extend(b.company_signer_id);
    let active: i64 = sqlx::query_scalar("SELECT COUNT(DISTINCT id) FROM users WHERE id = ANY($1) AND is_active").bind(&people).fetch_one(&state.db).await?;
    let mut uniq = people.clone();
    uniq.sort();
    uniq.dedup();
    if active as usize != uniq.len() {
        return Err(AppError::bad("everyone must be an active employee"));
    }
    // The source: a template, or a file made into a PDF once.
    let (body, file_pdf): (Option<String>, Option<Vec<u8>>) = match (b.template_id, b.file_id) {
        (Some(t), _) => (Some(sqlx::query_scalar("SELECT body FROM hr_doc_templates WHERE id = $1").bind(t).fetch_optional(&state.db).await?.ok_or_else(|| AppError::bad("no such template"))?), None),
        (None, Some(f)) => {
            let (name, mime, owner, bytes) = read_bytes(&state, f).await?;
            if owner != Some(auth.id) {
                return Err(AppError::bad("use a file you uploaded"));
            }
            let ext = name.rsplit_once('.').map(|(_, e)| e.to_lowercase()).unwrap_or_default();
            let pdf = if mime == "application/pdf" || ext == "pdf" {
                bytes
            } else if ["doc", "docx", "odt", "rtf"].contains(&ext.as_str()) {
                let c = &state.config.docs;
                crate::crm::docs::office::convert(&c.soffice_bin, &bytes, &ext, "pdf", c.timeout_secs).await.map_err(|e| AppError::bad(format!("the file could not be made into a PDF: {e}")))?
            } else {
                return Err(AppError::bad("use a PDF, Word or OpenDocument file"));
            };
            if !pdf.starts_with(b"%PDF") {
                return Err(AppError::bad("that isn't a PDF"));
            }
            (None, Some(pdf))
        }
        _ => return Err(AppError::bad("choose a template or a file")),
    };
    let batch = Uuid::new_v4();
    let mut made = Vec::new();
    for emp in &b.employee_ids {
        let pdf = match (&body, &file_pdf) {
            (Some(t), _) => render(&state, &title, t, *emp).await?,
            (_, Some(p)) => p.clone(),
            _ => unreachable!(),
        };
        let name: String = sqlx::query_scalar("SELECT trim(first_name || ' ' || last_name) FROM users WHERE id = $1").bind(emp).fetch_one(&state.db).await?;
        let fid = store_bytes(&state, None, &format!("{title} — {name}.pdf"), "application/pdf", &pdf).await?;
        let mut tx = state.db.begin().await?;
        let id: Uuid = sqlx::query_scalar(
            "INSERT INTO hr_docs (title, template_id, batch_id, employee_id, status, file_id, sha256, due_on, created_by)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) RETURNING id",
        )
        .bind(&title)
        .bind(b.template_id)
        .bind(batch)
        .bind(emp)
        .bind(if b.approver_ids.is_empty() { "signing" } else { "approval" })
        .bind(fid)
        .bind(sha256_hex(&pdf))
        .bind(b.due_on)
        .bind(auth.id)
        .fetch_one(&mut *tx)
        .await?;
        let mut pos = 0;
        for a in &b.approver_ids {
            sqlx::query("INSERT INTO hr_doc_steps (doc_id, position, user_id, role) VALUES ($1, $2, $3, 'approver')").bind(id).bind(pos).bind(a).execute(&mut *tx).await?;
            pos += 1;
        }
        if let Some(c) = b.company_signer_id.filter(|c| c != emp) {
            sqlx::query("INSERT INTO hr_doc_steps (doc_id, position, user_id, role) VALUES ($1, $2, $3, 'company')").bind(id).bind(pos).bind(c).execute(&mut *tx).await?;
            pos += 1;
        }
        sqlx::query("INSERT INTO hr_doc_steps (doc_id, position, user_id, role) VALUES ($1, $2, $3, 'employee')").bind(id).bind(pos).bind(emp).execute(&mut *tx).await?;
        tx.commit().await?;
        advance(&state, id).await?;
        made.push(id);
    }
    Ok(Json(json!({ "batch_id": batch, "ids": made })))
}

/// Gives the next step to its person (and tells them); completes the document when none is left.
async fn advance(state: &AppState, id: Uuid) -> AppResult<()> {
    let next: Option<(i32, Uuid, String)> = sqlx::query_as("SELECT position, user_id, role FROM hr_doc_steps WHERE doc_id = $1 AND status <> 'done' ORDER BY position LIMIT 1").bind(id).fetch_optional(&state.db).await?;
    let (title, emp_name): (String, String) = sqlx::query_as("SELECT d.title, trim(u.first_name || ' ' || u.last_name) FROM hr_docs d JOIN users u ON u.id = d.employee_id WHERE d.id = $1").bind(id).fetch_one(&state.db).await?;
    match next {
        Some((pos, user, role)) => {
            let n = sqlx::query("UPDATE hr_doc_steps SET status = 'pending' WHERE doc_id = $1 AND position = $2 AND status = 'waiting'").bind(id).bind(pos).execute(&state.db).await?.rows_affected();
            if role != "approver" {
                sqlx::query("UPDATE hr_docs SET status = 'signing' WHERE id = $1 AND status = 'approval'").bind(id).execute(&state.db).await?;
            }
            if n == 1 {
                let what = match role.as_str() {
                    "approver" => format!("Please approve “{title}” for {emp_name}"),
                    "company" => format!("Please sign “{title}” for the company ({emp_name})"),
                    _ => format!("Please sign “{title}”"),
                };
                notify(state, &[user], NewNotification { kind: "hr.document", title: what, body: None, link: Some(format!("/hr/documents/{id}")), actor: None }).await;
            }
        }
        None => finish(state, id).await?,
    }
    Ok(())
}

async fn finish(state: &AppState, id: Uuid) -> AppResult<()> {
    let claimed = sqlx::query("UPDATE hr_docs SET status = 'signed', completed_at = now() WHERE id = $1 AND status = 'signing'").bind(id).execute(&state.db).await?.rows_affected();
    if claimed == 0 {
        return Ok(());
    }
    let (title, file, sha, created, completed, employee, by): (String, Uuid, String, DateTime<Utc>, DateTime<Utc>, Uuid, Option<Uuid>) =
        sqlx::query_as("SELECT title, file_id, sha256, created_at, completed_at, employee_id, created_by FROM hr_docs WHERE id = $1").bind(id).fetch_one(&state.db).await?;
    let rows: Vec<(String, String, Option<String>, Option<DateTime<Utc>>, Option<String>, Option<String>, Option<Uuid>)> = sqlx::query_as(
        "SELECT trim(u.first_name || ' ' || u.last_name), u.email, s.signed_name, s.acted_at, s.ip, s.user_agent, s.signature_file_id
         FROM hr_doc_steps s JOIN users u ON u.id = s.user_id WHERE s.doc_id = $1 AND s.role <> 'approver' ORDER BY s.position",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    let list: Vec<CertSigner> = rows
        .into_iter()
        .map(|(name, email, sn, at, ip, ua, sig)| CertSigner { name, email, signed_name: sn.unwrap_or_default(), signed_at: at, ip, agent: ua, signature: sig })
        .collect();
    let original = read_bytes(state, file).await?.3;
    let signed = signed_copy(state, &title, &id.to_string(), &sha, created, completed, &list, &original).await?;
    let emp_name: String = sqlx::query_scalar("SELECT trim(first_name || ' ' || last_name) FROM users WHERE id = $1").bind(employee).fetch_one(&state.db).await?;
    let sf = store_bytes(state, None, &format!("{title} — {emp_name} (signed).pdf"), "application/pdf", &signed).await?;
    sqlx::query("UPDATE hr_docs SET signed_file_id = $2, signed_sha256 = $3 WHERE id = $1").bind(id).bind(sf).bind(sha256_hex(&signed)).execute(&state.db).await?;
    let mut tell = vec![employee];
    tell.extend(by);
    notify(state, &tell, NewNotification { kind: "hr.document", title: format!("“{title}” for {emp_name} is signed"), body: None, link: Some(format!("/hr/documents/{id}")), actor: None }).await;
    crate::api::webhooks::emit(state, "hr.document.signed", json!({ "id": id, "title": title, "employee_id": employee, "signed_sha256": sha256_hex(&signed) })).await;
    Ok(())
}

#[derive(sqlx::FromRow)]
struct Doc {
    id: Uuid,
    title: String,
    batch_id: Uuid,
    employee_id: Uuid,
    employee_name: String,
    status: String,
    sha256: String,
    signed_file_id: Option<Uuid>,
    signed_sha256: Option<String>,
    due_on: Option<NaiveDate>,
    created_at: DateTime<Utc>,
    completed_at: Option<DateTime<Utc>>,
    revoked_at: Option<DateTime<Utc>>,
    revoke_reason: Option<String>,
}

const SELECT: &str = "SELECT d.id, d.title, d.batch_id, d.employee_id, trim(u.first_name || ' ' || u.last_name) AS employee_name, d.status, d.sha256,
    d.signed_file_id, d.signed_sha256, d.due_on, d.created_at, d.completed_at, d.revoked_at, d.revoke_reason FROM hr_docs d JOIN users u ON u.id = d.employee_id";

async fn load(state: &AppState, id: Uuid) -> AppResult<Doc> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE d.id = $1"))).bind(id).fetch_optional(&state.db).await?.ok_or(AppError::NotFound("document"))
}

/// HR, the employee, and the people on its steps may see it.
async fn may_see(state: &AppState, auth: &AuthUser, d: &Doc) -> AppResult<bool> {
    if hr_manager(auth) || d.employee_id == auth.id {
        return Ok(true);
    }
    Ok(sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM hr_doc_steps WHERE doc_id = $1 AND user_id = $2)").bind(d.id).bind(auth.id).fetch_one(&state.db).await?)
}

fn overdue(d: &Doc) -> bool {
    d.due_on.is_some_and(|due| due < Utc::now().date_naive()) && ["approval", "signing"].contains(&d.status.as_str())
}

async fn doc_json(state: &AppState, d: &Doc, me: Uuid) -> AppResult<Value> {
    let steps: Vec<(i32, Uuid, String, String, String, Option<DateTime<Utc>>, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT s.position, s.user_id, trim(u.first_name || ' ' || u.last_name), s.role, s.status, s.acted_at, s.comment, s.signed_name
         FROM hr_doc_steps s JOIN users u ON u.id = s.user_id WHERE s.doc_id = $1 ORDER BY s.position",
    )
    .bind(d.id)
    .fetch_all(&state.db)
    .await?;
    let my_turn = steps.iter().find(|s| s.4 == "pending").filter(|s| s.1 == me).map(|s| s.3.clone());
    Ok(json!({
        "id": d.id, "title": d.title, "batch_id": d.batch_id, "employee_id": d.employee_id, "employee_name": d.employee_name, "status": d.status,
        "overdue": overdue(d), "sha256": d.sha256, "signed_sha256": d.signed_sha256, "signed": d.signed_file_id.is_some(), "due_on": d.due_on,
        "created_at": d.created_at, "completed_at": d.completed_at, "revoked_at": d.revoked_at, "revoke_reason": d.revoke_reason,
        "my_turn": my_turn,
        "steps": steps.into_iter().map(|(p, u, n, r, st, at, c, sn)| json!({ "position": p, "user_id": u, "name": n, "role": r, "status": st, "acted_at": at, "comment": c, "signed_name": sn })).collect::<Vec<_>>(),
    }))
}

#[derive(Deserialize)]
struct ListQuery {
    /// todo: my turn · mine: about me · all: everything (HR) · employee: one person's (HR)
    scope: Option<String>,
    employee_id: Option<Uuid>,
}

async fn list(State(state): State<AppState>, auth: AuthUser, Query(q): Query<ListQuery>) -> AppResult<Json<Value>> {
    let docs: Vec<Doc> = match q.scope.as_deref() {
        Some("todo") => sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "{SELECT} WHERE d.status IN ('approval', 'signing') AND EXISTS (SELECT 1 FROM hr_doc_steps s WHERE s.doc_id = d.id AND s.user_id = $1 AND s.status = 'pending') ORDER BY d.due_on NULLS LAST, d.created_at"
        )))
        .bind(auth.id)
        .fetch_all(&state.db)
        .await?,
        Some("all") => {
            need_hr(&auth)?;
            sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} ORDER BY d.created_at DESC LIMIT 1000"))).fetch_all(&state.db).await?
        }
        Some("employee") => {
            let e = q.employee_id.ok_or_else(|| AppError::bad("whose documents?"))?;
            if e != auth.id && !hr_manager(&auth) && !super::manages(&state, &auth, e).await? {
                return Err(AppError::Forbidden);
            }
            sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE d.employee_id = $1 ORDER BY d.created_at DESC"))).bind(e).fetch_all(&state.db).await?
        }
        _ => sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "{SELECT} WHERE d.employee_id = $1 OR EXISTS (SELECT 1 FROM hr_doc_steps s WHERE s.doc_id = d.id AND s.user_id = $1) ORDER BY d.created_at DESC LIMIT 500"
        )))
        .bind(auth.id)
        .fetch_all(&state.db)
        .await?,
    };
    let mut out = Vec::new();
    for d in &docs {
        out.push(doc_json(&state, d, auth.id).await?);
    }
    Ok(Json(json!({ "items": out })))
}

async fn one(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let d = load(&state, id).await?;
    if !may_see(&state, &auth, &d).await? {
        return Err(AppError::NotFound("document"));
    }
    Ok(Json(doc_json(&state, &d, auth.id).await?))
}

/// The PDF: the signed copy once there is one, else the document as sent.
async fn file(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Response> {
    let d = load(&state, id).await?;
    if !may_see(&state, &auth, &d).await? {
        return Err(AppError::NotFound("document"));
    }
    let fid: Option<Uuid> = sqlx::query_scalar("SELECT COALESCE(signed_file_id, file_id) FROM hr_docs WHERE id = $1").bind(id).fetch_one(&state.db).await?;
    let (_, _, _, bytes) = read_bytes(&state, fid.ok_or(AppError::NotFound("document"))?).await?;
    let mut r = bytes.into_response();
    let h = r.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/pdf"));
    h.insert(header::CONTENT_DISPOSITION, HeaderValue::from_static("inline; filename=\"document.pdf\""));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
    h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    Ok(r)
}

/// My pending step on a document that's still open.
async fn my_step(state: &AppState, auth: &AuthUser, id: Uuid, roles: &[&str]) -> AppResult<(Doc, i32, String)> {
    let d = load(state, id).await?;
    if !may_see(state, auth, &d).await? {
        return Err(AppError::NotFound("document"));
    }
    if !["approval", "signing"].contains(&d.status.as_str()) {
        return Err(AppError::bad("this document is no longer waiting"));
    }
    if overdue(&d) {
        return Err(AppError::bad("the due date has passed; ask HR to send it again"));
    }
    let step: Option<(i32, String)> = sqlx::query_as("SELECT position, role FROM hr_doc_steps WHERE doc_id = $1 AND user_id = $2 AND status = 'pending'").bind(id).bind(auth.id).fetch_optional(&state.db).await?;
    let (pos, role) = step.ok_or_else(|| AppError::bad("it isn't your turn on this document"))?;
    if !roles.contains(&role.as_str()) {
        return Err(AppError::bad(if role == "approver" { "approve or decline it" } else { "sign or decline it" }));
    }
    Ok((d, pos, role))
}

#[derive(Deserialize)]
struct ApproveInput {
    approve: bool,
    comment: Option<String>,
}

async fn approve(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<ApproveInput>) -> AppResult<Json<Value>> {
    let (d, pos, _) = my_step(&state, &auth, id, &["approver"]).await?;
    let comment = crate::routes::clean(b.comment).map(|c| c.chars().take(2000).collect::<String>());
    if b.approve {
        sqlx::query("UPDATE hr_doc_steps SET status = 'done', acted_at = now(), comment = $3 WHERE doc_id = $1 AND position = $2").bind(id).bind(pos).bind(&comment).execute(&state.db).await?;
        advance(&state, id).await?;
    } else {
        stop(&state, &d, pos, comment, &auth, "sent back").await?;
    }
    one(State(state), auth, Path(id)).await
}

/// A step declined: the document stops; HR who made it is told.
async fn stop(state: &AppState, d: &Doc, pos: i32, comment: Option<String>, auth: &AuthUser, verb: &str) -> AppResult<()> {
    sqlx::query("UPDATE hr_doc_steps SET status = 'declined', acted_at = now(), comment = $3 WHERE doc_id = $1 AND position = $2").bind(d.id).bind(pos).bind(&comment).execute(&state.db).await?;
    sqlx::query("UPDATE hr_docs SET status = 'declined' WHERE id = $1").bind(d.id).execute(&state.db).await?;
    let by: Option<Uuid> = sqlx::query_scalar("SELECT created_by FROM hr_docs WHERE id = $1").bind(d.id).fetch_one(&state.db).await?;
    let me: String = sqlx::query_scalar("SELECT trim(first_name || ' ' || last_name) FROM users WHERE id = $1").bind(auth.id).fetch_one(&state.db).await?;
    notify(state, &by.into_iter().collect::<Vec<_>>(), NewNotification { kind: "hr.document", title: format!("{me} {verb} “{}” ({})", d.title, d.employee_name), body: comment, link: Some(format!("/hr/documents/{}", d.id)), actor: Some(auth.id) }).await;
    Ok(())
}

#[derive(Deserialize)]
struct SignInput {
    signed_name: String,
    /// A PNG of the drawn signature, as a data: URL.
    signature: String,
    agree: bool,
}

async fn sign(State(state): State<AppState>, auth: AuthUser, ClientIp(ip): ClientIp, headers: HeaderMap, Path(id): Path<Uuid>, Json(b): Json<SignInput>) -> AppResult<Json<Value>> {
    let (_, pos, _) = my_step(&state, &auth, id, &["company", "employee"]).await?;
    if !b.agree {
        return Err(AppError::bad("please agree to sign electronically"));
    }
    let typed = required("your name", &b.signed_name, 200)?;
    let png = b
        .signature
        .strip_prefix("data:image/png;base64,")
        .and_then(|d| base64::engine::general_purpose::STANDARD.decode(d).ok())
        .filter(|p| p.starts_with(b"\x89PNG\r\n\x1a\n"))
        .ok_or_else(|| AppError::bad("please draw your signature"))?;
    if png.len() > 300_000 {
        return Err(AppError::bad("the signature image is too large"));
    }
    let sig = store_bytes(&state, None, &format!("signature-{id}-{pos}.png"), "image/png", &png).await?;
    let ua: Option<String> = headers.get(header::USER_AGENT).and_then(|v| v.to_str().ok()).map(|v| v.chars().take(300).collect());
    let n = sqlx::query(
        "UPDATE hr_doc_steps SET status = 'done', acted_at = now(), signed_name = $3, signature_file_id = $4, ip = $5, user_agent = $6
         WHERE doc_id = $1 AND position = $2 AND status = 'pending'",
    )
    .bind(id)
    .bind(pos)
    .bind(&typed)
    .bind(sig)
    .bind(&ip)
    .bind(&ua)
    .execute(&state.db)
    .await?
    .rows_affected();
    if n == 1 {
        advance(&state, id).await?;
    }
    one(State(state), auth, Path(id)).await
}

#[derive(Deserialize)]
struct DeclineInput {
    reason: Option<String>,
}

async fn decline(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<DeclineInput>) -> AppResult<Json<Value>> {
    let (d, pos, _) = my_step(&state, &auth, id, &["company", "employee"]).await?;
    stop(&state, &d, pos, crate::routes::clean(b.reason).map(|c| c.chars().take(2000).collect()), &auth, "declined to sign").await?;
    one(State(state), auth, Path(id)).await
}

#[derive(Deserialize)]
struct RevokeInput {
    reason: String,
}

/// HR stops a document, or invalidates a signed one, with a reason; those on it are told.
async fn revoke(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<RevokeInput>) -> AppResult<Json<Value>> {
    need_hr(&auth)?;
    let reason = required("reason", &b.reason, 2000)?;
    let d = load(&state, id).await?;
    if d.status == "revoked" {
        return Err(AppError::bad("it is invalid already"));
    }
    sqlx::query("UPDATE hr_docs SET status = 'revoked', revoked_at = now(), revoked_by = $2, revoke_reason = $3 WHERE id = $1").bind(id).bind(auth.id).bind(&reason).execute(&state.db).await?;
    let mut people: Vec<Uuid> = sqlx::query_scalar("SELECT DISTINCT user_id FROM hr_doc_steps WHERE doc_id = $1").bind(id).fetch_all(&state.db).await?;
    people.retain(|p| *p != auth.id);
    let verb = if d.status == "signed" { "is no longer valid" } else { "was withdrawn" };
    notify(&state, &people, NewNotification { kind: "hr.document", title: format!("“{}” {verb}", d.title), body: Some(reason), link: Some(format!("/hr/documents/{id}")), actor: Some(auth.id) }).await;
    crate::api::webhooks::emit(&state, "hr.document.revoked", json!({ "id": id, "title": d.title, "employee_id": d.employee_id })).await;
    one(State(state), auth, Path(id)).await
}

/// Anyone can check whether a PDF's fingerprint (SHA-256) is a document signed here, and
/// whether it is still valid.
async fn verify(State(state): State<AppState>, Path(sha): Path<String>) -> AppResult<Json<Value>> {
    let sha = sha.trim().to_lowercase();
    if sha.len() != 64 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(AppError::bad("give the SHA-256 of the PDF (64 hex characters)"));
    }
    let hr: Option<(String, String, Option<DateTime<Utc>>, Option<DateTime<Utc>>, bool)> =
        sqlx::query_as("SELECT title, status, completed_at, revoked_at, signed_sha256 = $1 FROM hr_docs WHERE signed_sha256 = $1 OR sha256 = $1 LIMIT 1").bind(&sha).fetch_optional(&state.db).await?;
    if let Some((title, status, done, revoked, is_signed)) = hr {
        return Ok(Json(json!({ "found": true, "title": title, "signed_copy": is_signed, "status": status, "completed_at": done, "revoked_at": revoked, "valid": status == "signed" && is_signed })));
    }
    let crm: Option<(String, String, Option<DateTime<Utc>>, bool)> =
        sqlx::query_as("SELECT title, status, completed_at, signed_sha256 = $1 FROM sign_requests WHERE signed_sha256 = $1 OR original_sha256 = $1 LIMIT 1").bind(&sha).fetch_optional(&state.db).await?;
    if let Some((title, status, done, is_signed)) = crm {
        return Ok(Json(json!({ "found": true, "title": title, "signed_copy": is_signed, "status": status, "completed_at": done, "valid": status == "completed" && is_signed })));
    }
    Ok(Json(json!({ "found": false })))
}
