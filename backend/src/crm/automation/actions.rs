//! What robots (and schedules) can do. `validate` checks an action's parameters when it is
//! saved; `execute` runs it against a record and says what happened.

use redis::AsyncCommands;
use serde_json::{json, Map, Value};
use uuid::Uuid;

use super::{fill, fill_json, system_auth, Ctx, Robot, SYSTEM_USER};
use crate::{
    crm::{
        broadcast, entity_type_by_id,
        fields::{custom_fields, system_fields_for},
        items::{create_item, update_item, Convert},
        timeline, COMPANY, CONTACT, DEAL, LEAD,
    },
    error::{AppError, AppResult},
    jobs, mail,
    notify::{notify, NewNotification},
    state::AppState,
};

pub const ACTIONS: &[(&str, &str)] = &[
    ("activity", "Plan an activity"),
    ("task", "Create a task"),
    ("notify", "Notify employees"),
    ("email", "Email the client"),
    ("set_field", "Change a field"),
    ("assign", "Change the responsible person"),
    ("move_stage", "Move to another stage"),
    ("tunnel", "Tunnel to another pipeline"),
    ("document", "Create a document"),
    ("sign", "Send for signature"),
    ("comment", "Add a comment"),
    ("webhook", "Call a webhook"),
    ("create_record", "Create a linked record"),
    ("convert_lead", "Convert the lead"),
    ("observers", "Add observers"),
];

pub fn label(action: &str) -> &'static str {
    ACTIONS.iter().find(|a| a.0 == action).map(|a| a.1).unwrap_or("Robot")
}

/// What an action did.
pub enum Step {
    Done(String),
    /// The action started a document and continues when its PDF is ready.
    AwaitDocument(Uuid),
}

/// Why an action failed, and whether trying again later may help.
pub struct Fail {
    pub msg: String,
    pub retry: bool,
}

impl From<String> for Fail {
    fn from(msg: String) -> Self {
        Fail { msg, retry: false }
    }
}
impl From<&str> for Fail {
    fn from(msg: &str) -> Self {
        Fail { msg: msg.into(), retry: false }
    }
}

fn obj(p: Value) -> AppResult<Map<String, Value>> {
    match p {
        Value::Object(m) => Ok(m),
        Value::Null => Ok(Map::new()),
        _ => Err(AppError::bad("params must be an object")),
    }
}

fn text(p: &Map<String, Value>, key: &str, label: &str, max: usize, required: bool) -> AppResult<String> {
    let s = p.get(key).and_then(Value::as_str).unwrap_or("").trim().to_string();
    if required && s.is_empty() {
        return Err(AppError::bad(format!("{label} is required")));
    }
    if s.chars().count() > max {
        return Err(AppError::bad(format!("{label} is too long")));
    }
    Ok(s)
}

fn flag(p: &Map<String, Value>, key: &str) -> bool {
    p.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// "assigned", "creator" or an active employee's id.
async fn check_person(state: &AppState, v: Option<&Value>) -> AppResult<String> {
    let s = v.and_then(Value::as_str).unwrap_or("assigned");
    if s == "assigned" || s == "creator" {
        return Ok(s.into());
    }
    let id = Uuid::parse_str(s).map_err(|_| AppError::bad("choose who: the responsible person, the creator or an employee"))?;
    let ok: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users WHERE id = $1 AND is_active AND NOT is_system)").bind(id).fetch_one(&state.db).await?;
    if !ok {
        return Err(AppError::bad("that employee is not active"));
    }
    Ok(s.into())
}

fn minutes(p: &Map<String, Value>, key: &str) -> AppResult<i64> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(0),
        Some(v) => {
            let n = v.as_i64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok())).ok_or_else(|| AppError::bad(format!("{key} must be a number of minutes")))?;
            if !(0..=525_600).contains(&n) {
                return Err(AppError::bad(format!("{key} must be within a year")));
            }
            Ok(n)
        }
    }
}

fn opt_uuid(p: &Map<String, Value>, key: &str, msg: &str) -> AppResult<Option<Uuid>> {
    match p.get(key).and_then(Value::as_str).filter(|s| !s.is_empty()) {
        Some(s) => Ok(Some(Uuid::parse_str(s).map_err(|_| AppError::bad(msg.to_string()))?)),
        None => Ok(None),
    }
}

/// A copy of the record in another pipeline's stage, linked to it, with its client, amount,
/// custom fields and products.
async fn tunnel_copy(state: &AppState, et: &crate::crm::EntityType, source: Uuid, category: Uuid, stage: Uuid) -> AppResult<Uuid> {
    let (title, assigned, company, currency, amount, manual, uf, comments): (String, Option<Uuid>, Option<Uuid>, Option<String>, rust_decimal::Decimal, bool, Value, Option<String>) = sqlx::query_as(
        "SELECT title, assigned_to, company_id, currency, opportunity, is_manual_amount, uf, comments FROM crm_items WHERE id = $1",
    )
    .bind(source)
    .fetch_one(&state.db)
    .await?;
    let contacts: Vec<Uuid> = sqlx::query_scalar("SELECT contact_id FROM crm_item_contacts WHERE item_id = $1 ORDER BY is_primary DESC, sort").bind(source).fetch_all(&state.db).await?;
    let mut m = Map::new();
    m.insert("title".into(), json!(title));
    m.insert("assigned_to".into(), json!(assigned));
    m.insert("category_id".into(), json!(category));
    m.insert("stage_id".into(), json!(stage));
    if et.has_client {
        m.insert("company_id".into(), json!(company));
        m.insert("contact_ids".into(), json!(contacts));
    }
    if et.has_amount {
        m.insert("currency".into(), json!(currency));
        m.insert("opportunity".into(), json!(amount.to_string()));
        m.insert("is_manual_amount".into(), json!(manual));
    }
    if uf.as_object().is_some_and(|o| !o.is_empty()) {
        m.insert("uf".into(), uf);
    }
    if let Some(c) = comments {
        m.insert("comments".into(), json!(c));
    }
    let id = create_item(state, &system_auth(), et, m, Some(source)).await?;
    sqlx::query(
        "INSERT INTO crm_item_products (item_id, product_id, name, price, quantity, unit, discount_kind, discount_value, tax_rate, tax_included, sort, cost)
         SELECT $1, product_id, name, price, quantity, unit, discount_kind, discount_value, tax_rate, tax_included, sort, cost FROM crm_item_products WHERE item_id = $2",
    )
    .bind(id)
    .bind(source)
    .execute(&state.db)
    .await?;
    crate::crm::money::totals(state, id).await?;
    timeline(&state.db, source, "automation", Some(SYSTEM_USER), None, json!({ "event": "tunnel", "item_id": id })).await?;
    broadcast(state, et.id, id, false).await;
    Ok(id)
}

async fn check_template(state: &AppState, entity_type: i16, t: Uuid) -> AppResult<()> {
    let ok: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM crm_doc_templates WHERE id = $1 AND $2 = ANY(entity_types))")
        .bind(t)
        .bind(entity_type)
        .fetch_one(&state.db)
        .await?;
    if ok { Ok(()) } else { Err(AppError::bad("that template is not offered for this record type")) }
}

const METHODS: &[&str] = &["POST", "PUT", "PATCH", "GET", "DELETE"];

use crate::outbound::check_url;

/// Checks and normalises an action's parameters.
pub async fn validate(state: &AppState, entity_type: i16, category: Uuid, action: &str, params: Value) -> AppResult<Value> {
    let p = obj(params)?;
    let out = match action {
        "activity" => {
            let kind = p.get("kind").and_then(Value::as_str).unwrap_or("todo");
            if !["todo", "call", "meeting"].contains(&kind) {
                return Err(AppError::bad("activity kind must be todo, call or meeting"));
            }
            json!({
                "kind": kind,
                "subject": text(&p, "subject", "Subject", 500, true)?,
                "description": text(&p, "description", "Description", 5000, false)?,
                "deadline_minutes": minutes(&p, "deadline_minutes")?,
                "responsible": check_person(state, p.get("responsible")).await?,
            })
        }
        "task" => {
            let project = opt_uuid(&p, "project_id", "choose a project")?;
            if let Some(id) = project {
                let ok: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM projects WHERE id = $1)").bind(id).fetch_one(&state.db).await?;
                if !ok {
                    return Err(AppError::bad("that project does not exist"));
                }
            }
            json!({
                "title": text(&p, "title", "Task title", 500, true)?,
                "description": text(&p, "description", "Description", 10_000, false)?,
                "deadline_minutes": minutes(&p, "deadline_minutes")?,
                "responsible": check_person(state, p.get("responsible")).await?,
                "project_id": project,
            })
        }
        "notify" => {
            let to = p.get("to").and_then(Value::as_array).cloned().unwrap_or_else(|| vec![json!("assigned")]);
            if to.is_empty() || to.len() > 50 {
                return Err(AppError::bad("notify between 1 and 50 people"));
            }
            let mut people = Vec::new();
            for t in &to {
                people.push(check_person(state, Some(t)).await?);
            }
            json!({ "to": people, "message": text(&p, "message", "Message", 2000, true)? })
        }
        "email" => {
            let template = opt_uuid(&p, "template_id", "choose a template")?;
            if let Some(t) = template {
                check_template(state, entity_type, t).await?;
            }
            json!({
                "subject": text(&p, "subject", "Subject", 300, true)?,
                "body": text(&p, "body", "Message", 20_000, true)?,
                "template_id": template,
            })
        }
        "comment" => json!({ "text": text(&p, "text", "Comment", 20_000, true)? }),
        "set_field" => {
            let field = text(&p, "field", "Field", 100, true)?;
            let known = match field.strip_prefix("uf.") {
                Some(code) => custom_fields(state, entity_type).await?.iter().any(|f| f.code == code),
                None => system_fields_for(&entity_type_by_id(state, entity_type).await?).iter().any(|f| f.code == field && f.kind != "multifield" && f.kind != "address" && f.code != "assigned_to"),
            };
            if !known {
                return Err(AppError::bad("choose a field of this record type"));
            }
            json!({ "field": field, "value": p.get("value").cloned().unwrap_or(Value::Null) })
        }
        "assign" => {
            let users = p.get("users").and_then(Value::as_array).cloned().unwrap_or_default();
            if users.is_empty() || users.len() > 100 {
                return Err(AppError::bad("choose between 1 and 100 employees"));
            }
            let mut ids = Vec::new();
            for u in &users {
                let s = check_person(state, Some(u)).await?;
                if s == "assigned" || s == "creator" {
                    return Err(AppError::bad("choose employees"));
                }
                ids.push(s);
            }
            let mode = p.get("mode").and_then(Value::as_str).unwrap_or("round_robin");
            if !["round_robin", "random", "least_busy"].contains(&mode) {
                return Err(AppError::bad("mode must be round_robin, random or least_busy"));
            }
            json!({ "users": ids, "mode": mode })
        }
        "move_stage" => {
            let s = p.get("stage_id").and_then(Value::as_str).and_then(|s| Uuid::parse_str(s).ok()).ok_or_else(|| AppError::bad("choose a stage"))?;
            let ok: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM crm_stages WHERE id = $1 AND category_id = $2)").bind(s).bind(category).fetch_one(&state.db).await?;
            if !ok {
                return Err(AppError::bad("choose a stage of this pipeline"));
            }
            json!({ "stage_id": s })
        }
        "document" => {
            let t = p.get("template_id").and_then(Value::as_str).and_then(|s| Uuid::parse_str(s).ok()).ok_or_else(|| AppError::bad("choose a template"))?;
            check_template(state, entity_type, t).await?;
            json!({ "template_id": t })
        }
        "tunnel" => {
            // A stage of another pipeline of the same record type; the record is copied there
            // (linked to this one) or moved there.
            let s = p.get("stage_id").and_then(Value::as_str).and_then(|s| Uuid::parse_str(s).ok()).ok_or_else(|| AppError::bad("choose the stage to tunnel to"))?;
            let target: Option<(Uuid, i16)> = sqlx::query_as("SELECT s.category_id, c.entity_type FROM crm_stages s JOIN crm_categories c ON c.id = s.category_id WHERE s.id = $1").bind(s).fetch_optional(&state.db).await?;
            match target {
                Some((c, et)) if et == entity_type && c != category => {}
                Some(_) => return Err(AppError::bad("tunnel to a stage of another pipeline of the same record type")),
                None => return Err(AppError::bad("that stage doesn't exist")),
            }
            let mode = p.get("mode").and_then(Value::as_str).unwrap_or("copy");
            if !["copy", "move"].contains(&mode) {
                return Err(AppError::bad("a tunnel copies or moves the record"));
            }
            json!({ "stage_id": s, "mode": mode })
        }
        "sign" => {
            let t = p.get("sign_template_id").and_then(Value::as_str).and_then(|s| Uuid::parse_str(s).ok()).ok_or_else(|| AppError::bad("choose a signing template"))?;
            let ok: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM sign_templates WHERE id = $1 AND (cardinality(entity_types) = 0 OR $2 = ANY(entity_types)))").bind(t).bind(entity_type).fetch_one(&state.db).await?;
            if !ok {
                return Err(AppError::bad("that signing template is not offered for this record type"));
            }
            json!({ "sign_template_id": t })
        }
        "webhook" => {
            let url = text(&p, "url", "URL", 2000, true)?;
            check_url(&url.replace("{{", "x").replace("}}", "x")).map_err(AppError::bad)?;
            let method = p.get("method").and_then(Value::as_str).unwrap_or("POST").to_uppercase();
            if !METHODS.contains(&method.as_str()) {
                return Err(AppError::bad(format!("method must be one of {}", METHODS.join(", "))));
            }
            let mut headers = Vec::new();
            for h in p.get("headers").and_then(Value::as_array).cloned().unwrap_or_default() {
                let name = h.get("name").and_then(Value::as_str).unwrap_or("").trim().to_string();
                let value = h.get("value").and_then(Value::as_str).unwrap_or("").trim().to_string();
                if name.is_empty() && value.is_empty() {
                    continue;
                }
                if name.is_empty() || name.len() > 100 || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b)) {
                    return Err(AppError::bad("header names may use letters, digits, - and _"));
                }
                if ["host", "content-length", "transfer-encoding", "connection"].contains(&name.to_lowercase().as_str()) {
                    return Err(AppError::bad(format!("the {name} header is set automatically")));
                }
                if value.len() > 2000 || value.contains(['\r', '\n']) {
                    return Err(AppError::bad("header values must be one line"));
                }
                headers.push(json!({ "name": name, "value": value }));
            }
            if headers.len() > 10 {
                return Err(AppError::bad("at most 10 headers"));
            }
            json!({
                "url": url,
                "method": method,
                "headers": headers,
                "body": text(&p, "body", "Body", 20_000, false)?,
                "secret": text(&p, "secret", "Signing secret", 200, false)?,
            })
        }
        "create_record" => {
            let target = p.get("entity_type").and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))).ok_or_else(|| AppError::bad("choose the type of record to create"))? as i16;
            let tt = entity_type_by_id(state, target).await.map_err(|_| AppError::bad("choose the type of record to create"))?;
            if !(tt.id == DEAL || tt.id == LEAD || tt.is_custom()) {
                return Err(AppError::bad("robots can create leads, deals and smart-process records"));
            }
            let cat = opt_uuid(&p, "category_id", "choose a pipeline")?;
            if let Some(c) = cat {
                let ok: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM crm_categories WHERE id = $1 AND entity_type = $2)").bind(c).bind(tt.id).fetch_one(&state.db).await?;
                if !ok {
                    return Err(AppError::bad("choose a pipeline of that record type"));
                }
            }
            json!({
                "entity_type": tt.id,
                "category_id": cat,
                "title": text(&p, "title", "Title", 300, true)?,
                "responsible": check_person(state, p.get("responsible")).await?,
                "copy_client": flag(&p, "copy_client"),
                "copy_amount": flag(&p, "copy_amount"),
            })
        }
        "observers" => {
            let users = p.get("users").and_then(Value::as_array).cloned().unwrap_or_default();
            if users.is_empty() || users.len() > 50 {
                return Err(AppError::bad("choose between 1 and 50 people"));
            }
            let mut out = Vec::new();
            for u in &users {
                out.push(check_person(state, Some(u)).await?);
            }
            json!({ "users": out })
        }
        "convert_lead" => {
            if entity_type != LEAD {
                return Err(AppError::bad("only leads can be converted"));
            }
            let (contact, company, deal) = (flag(&p, "create_contact"), flag(&p, "create_company"), flag(&p, "create_deal"));
            if !(contact || company || deal) {
                return Err(AppError::bad("choose what to create from the lead"));
            }
            let cat = opt_uuid(&p, "deal_category_id", "choose a deal pipeline")?;
            if let Some(c) = cat {
                let ok: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM crm_categories WHERE id = $1 AND entity_type = $2)").bind(c).bind(DEAL).fetch_one(&state.db).await?;
                if !ok {
                    return Err(AppError::bad("choose a deal pipeline"));
                }
            }
            json!({ "create_contact": contact, "create_company": company, "create_deal": deal, "deal_category_id": cat })
        }
        _ => return Err(AppError::bad(format!("action must be one of {}", ACTIONS.iter().map(|a| a.0).collect::<Vec<_>>().join(", ")))),
    };
    Ok(out)
}

fn person(ctx: &Ctx, v: &Value) -> Result<Uuid, String> {
    match v.as_str().unwrap_or("assigned") {
        "assigned" => ctx.row.assigned_to.ok_or_else(|| "the record has no responsible person".to_string()),
        "creator" => ctx.row.created_by.filter(|u| *u != SYSTEM_USER).ok_or_else(|| "the record's creator is unknown".to_string()),
        s => Uuid::parse_str(s).map_err(|e| e.to_string()),
    }
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

async fn active(state: &AppState, u: Uuid) -> Result<bool, String> {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users WHERE id = $1 AND is_active)").bind(u).fetch_one(&state.db).await.map_err(err)
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Runs an action. `data` is what a paused run stored (e.g. the document it waited for).
pub async fn execute(state: &AppState, robot: &Robot, ctx: &Ctx, data: &Value) -> Result<Step, Fail> {
    let p = &robot.params;
    let item = ctx.row.id;
    let f = |key: &str| fill(p.get(key).and_then(Value::as_str).unwrap_or(""), &ctx.value);
    let et = entity_type_by_id(state, ctx.row.entity_type).await.map_err(err)?;
    let done = |s: String| Ok(Step::Done(s));
    match robot.action.as_str() {
        "activity" => {
            let who = person(ctx, &p["responsible"])?;
            let kind = p["kind"].as_str().unwrap_or("todo");
            let subject = f("subject");
            let deadline = chrono::Utc::now() + chrono::Duration::minutes(p["deadline_minutes"].as_i64().unwrap_or(0));
            let id: Uuid = sqlx::query_scalar(
                "INSERT INTO crm_activities (item_id, kind, subject, description, starts_at, deadline, responsible_id, created_by, direction)
                 VALUES ($1, $2, $3, NULLIF($4, ''), CASE WHEN $2 = 'meeting' THEN $5 END, $5, $6, $7, CASE WHEN $2 = 'call' THEN 'outgoing' END) RETURNING id",
            )
            .bind(item)
            .bind(kind)
            .bind(&subject)
            .bind(f("description"))
            .bind(deadline)
            .bind(who)
            .bind(SYSTEM_USER)
            .fetch_one(&state.db)
            .await
            .map_err(err)?;
            timeline(&state.db, item, "activity", Some(SYSTEM_USER), Some(subject.clone()), json!({ "activity_id": id, "kind": kind, "event": "created" })).await.map_err(err)?;
            notify(state, &[who], NewNotification {
                kind: "crm.activity",
                title: format!("New {kind} for you: {subject}"),
                body: Some(format!("{}: {}", et.name, ctx.row.title)),
                link: Some(format!("/crm/{}/{item}", et.code)),
                actor: Some(SYSTEM_USER),
            })
            .await;
            done(format!("planned a {kind}: {subject}"))
        }
        "task" => {
            let who = person(ctx, &p["responsible"])?;
            if !active(state, who).await? {
                return Err("the responsible person is not active".into());
            }
            let mut description = f("description");
            description.push_str(&format!("{}{}: {}", if description.is_empty() { "" } else { "\n\n" }, et.name, ctx.value["link"].as_str().unwrap_or("")));
            let minutes = p["deadline_minutes"].as_i64().unwrap_or(0);
            let body = crate::tasks::items::CreateTask {
                title: f("title"),
                description: Some(description),
                responsible_id: Some(who),
                deadline: (minutes > 0).then(|| chrono::Utc::now() + chrono::Duration::minutes(minutes)),
                project_id: p["project_id"].as_str().and_then(|s| Uuid::parse_str(s).ok()),
                crm_item_ids: vec![item],
                ..Default::default()
            };
            let title = body.title.clone();
            crate::tasks::items::create_task(state, &system_auth(), body).await.map_err(err)?;
            done(format!("created the task “{title}”"))
        }
        "notify" => {
            let mut to = Vec::new();
            for v in p["to"].as_array().cloned().unwrap_or_default() {
                if let Ok(u) = person(ctx, &v)
                    && !to.contains(&u) {
                        to.push(u);
                    }
            }
            if to.is_empty() {
                return Err("nobody to notify".into());
            }
            let message = f("message");
            notify(state, &to, NewNotification {
                kind: "crm.robot",
                title: message.clone(),
                body: Some(format!("{}: {}", et.name, ctx.row.title)),
                link: Some(format!("/crm/{}/{item}", et.code)),
                actor: Some(SYSTEM_USER),
            })
            .await;
            done(format!("notified {} {}", to.len(), if to.len() == 1 { "person" } else { "people" }))
        }
        "email" => {
            let mut files = Vec::new();
            let mut number = None;
            if let Some(t) = p["template_id"].as_str().and_then(|s| Uuid::parse_str(s).ok()) {
                // First the document, then (once its PDF exists) the email.
                let doc = match data.get("document_id").and_then(Value::as_str).and_then(|s| Uuid::parse_str(s).ok()) {
                    Some(d) => d,
                    None => crate::crm::docs::api::generate(state, &system_auth(), item, t).await.map_err(err)?,
                };
                let (status, pdf, num, error): (String, Option<Uuid>, String, Option<String>) =
                    sqlx::query_as("SELECT status, pdf_file_id, number, error FROM crm_documents WHERE id = $1").bind(doc).fetch_one(&state.db).await.map_err(err)?;
                match (status.as_str(), pdf) {
                    ("ready", Some(pdf)) => files.push(pdf),
                    ("failed", _) => return Err(format!("the document {num} failed: {}", error.unwrap_or_default()).into()),
                    ("ready", None) => return Err(format!("the document {num} has no PDF").into()),
                    _ => return Ok(Step::AwaitDocument(doc)),
                }
                number = Some(num);
            }
            let to = ctx.value["client_email"].as_str().unwrap_or("").to_string();
            if to.is_empty() {
                return Err("the client has no email address".into());
            }
            let subject = f("subject");
            let body = f("body");
            let with = number.map(|n| format!(" with {n}")).unwrap_or_default();
            // From the responsible person's own mailbox when they connected one, so the
            // client's reply comes back into the CRM conversation.
            if let Some(owner) = ctx.row.assigned_to
                && let Some(mb) = crate::crm::email::mailbox_of(state, owner).await.map_err(err)? {
                    crate::crm::email::queue_send(state, owner, &mb, item, std::slice::from_ref(&to), &[], &subject, &body, None, &files).await.map_err(err)?;
                    return done(format!("emailed “{subject}”{with} to {to} from {}", mb.email));
                }
            let html = format!("<div style=\"font-family:sans-serif;font-size:14px;line-height:1.5\">{}</div>", html_escape(&body).replace('\n', "<br>"));
            jobs::send_email(state, mail::Email { to: to.clone(), subject: subject.clone(), text: body.clone(), html, attachments: files, calendar: None }).await.map_err(err)?;
            let id: Uuid = sqlx::query_scalar(
                "INSERT INTO crm_activities (item_id, kind, subject, description, direction, responsible_id, created_by, is_completed, completed_at)
                 VALUES ($1, 'email', $2, $3, 'outgoing', COALESCE($4, $5), $5, TRUE, now()) RETURNING id",
            )
            .bind(item)
            .bind(&subject)
            .bind(&body)
            .bind(ctx.row.assigned_to)
            .bind(SYSTEM_USER)
            .fetch_one(&state.db)
            .await
            .map_err(err)?;
            timeline(&state.db, item, "activity", Some(SYSTEM_USER), Some(subject.clone()), json!({ "activity_id": id, "kind": "email", "event": "completed", "result": format!("sent to {to}") })).await.map_err(err)?;
            sqlx::query("UPDATE crm_items SET last_activity_at = now() WHERE id = $1").bind(item).execute(&state.db).await.map_err(err)?;
            done(format!("emailed “{subject}”{with} to {to}"))
        }
        "comment" => {
            let text = f("text");
            timeline(&state.db, item, "comment", Some(SYSTEM_USER), Some(text), json!({})).await.map_err(err)?;
            done("added a comment".into())
        }
        "set_field" => {
            let field = p["field"].as_str().unwrap_or("");
            // Text values may use placeholders too.
            let value = match &p["value"] {
                Value::String(s) => Value::String(fill(s, &ctx.value)),
                other => other.clone(),
            };
            let mut body = Map::new();
            match field.strip_prefix("uf.") {
                Some(code) => {
                    body.insert("uf".into(), json!({ code: value }));
                }
                None => {
                    body.insert(field.into(), value.clone());
                }
            }
            update_item(state, &system_auth(), &et, item, body).await.map_err(err)?;
            done(format!("set {field} to {}", super::as_text(Some(&value))))
        }
        "assign" => {
            let users: Vec<Uuid> = p["users"].as_array().into_iter().flatten().filter_map(|v| v.as_str().and_then(|s| Uuid::parse_str(s).ok())).collect();
            let mut candidates = Vec::new();
            for u in users {
                if active(state, u).await? {
                    candidates.push(u);
                }
            }
            if candidates.is_empty() {
                return Err("none of the chosen employees is active".into());
            }
            let who = match p["mode"].as_str() {
                Some("random") => candidates[rand::random_range(0..candidates.len())],
                Some("least_busy") => {
                    // Fewest open records of this type; ties go to the earlier in the list.
                    let load: Vec<(Uuid, i64)> = sqlx::query_as(
                        "SELECT assigned_to, COUNT(*)::int8 FROM crm_items WHERE entity_type = $1 AND assigned_to = ANY($2) AND deleted_at IS NULL AND NOT is_closed AND id <> $3 GROUP BY assigned_to",
                    )
                    .bind(et.id)
                    .bind(&candidates)
                    .bind(item)
                    .fetch_all(&state.db)
                    .await
                    .map_err(err)?;
                    let count = |u: &Uuid| load.iter().find(|l| l.0 == *u).map(|l| l.1).unwrap_or(0);
                    *candidates.iter().min_by_key(|u| count(u)).unwrap()
                }
                _ => {
                    // Round robin, shared across all instances.
                    let mut redis = state.redis.clone();
                    let n: i64 = redis.incr(format!("crm:robot:rr:{}", robot.id), 1).await.map_err(err)?;
                    candidates[((n - 1) as usize) % candidates.len()]
                }
            };
            let mut body = Map::new();
            body.insert("assigned_to".into(), json!(who));
            update_item(state, &system_auth(), &et, item, body).await.map_err(err)?;
            let name: String = sqlx::query_scalar("SELECT first_name || ' ' || last_name FROM users WHERE id = $1").bind(who).fetch_one(&state.db).await.map_err(err)?;
            done(format!("made {name} responsible"))
        }
        "move_stage" => {
            let mut body = Map::new();
            body.insert("stage_id".into(), p["stage_id"].clone());
            update_item(state, &system_auth(), &et, item, body).await.map_err(err)?;
            done("moved the record".into())
        }
        "tunnel" => {
            let stage = p["stage_id"].as_str().and_then(|s| Uuid::parse_str(s).ok()).ok_or("no stage")?;
            let (category, stage_name, pipeline): (Uuid, String, String) = sqlx::query_as("SELECT s.category_id, s.name, c.name FROM crm_stages s JOIN crm_categories c ON c.id = s.category_id WHERE s.id = $1")
                .bind(stage)
                .fetch_optional(&state.db)
                .await
                .map_err(err)?
                .ok_or("the stage was deleted")?;
            if p["mode"] == "move" {
                let mut body = Map::new();
                body.insert("category_id".into(), json!(category));
                body.insert("stage_id".into(), json!(stage));
                update_item(state, &system_auth(), &et, item, body).await.map_err(err)?;
                done(format!("moved to {pipeline}: {stage_name}"))
            } else {
                let id = tunnel_copy(state, &et, item, category, stage).await.map_err(err)?;
                let number: i64 = sqlx::query_scalar("SELECT number FROM crm_items WHERE id = $1").bind(id).fetch_one(&state.db).await.map_err(err)?;
                done(format!("copied to {pipeline}: {stage_name} as #{number}"))
            }
        }
        "document" => {
            let t = p["template_id"].as_str().and_then(|s| Uuid::parse_str(s).ok()).ok_or("no template")?;
            let id = crate::crm::docs::api::generate(state, &system_auth(), item, t).await.map_err(err)?;
            let number: String = sqlx::query_scalar("SELECT number FROM crm_documents WHERE id = $1").bind(id).fetch_one(&state.db).await.map_err(err)?;
            broadcast(state, et.id, item, false).await;
            done(format!("created {number}"))
        }
        "sign" => {
            // Sent in the name of the record's responsible person.
            let t = p["sign_template_id"].as_str().and_then(|s| Uuid::parse_str(s).ok()).ok_or("no signing template")?;
            let by = ctx.row.assigned_to.ok_or("the record has no responsible person to send it")?;
            let r = crate::crm::sign_templates::start(state, by, item, t, None).await.map_err(err)?;
            done(if r["status"] == "sent" { "sent for signature".to_string() } else { "the document is being made; it goes out for signature when ready".to_string() })
        }
        "webhook" => call_webhook(state, robot, ctx).await.map(Step::Done),
        "create_record" => {
            let tt = entity_type_by_id(state, p["entity_type"].as_i64().unwrap_or(0) as i16).await.map_err(err)?;
            let mut m = Map::new();
            m.insert("title".into(), json!(f("title")));
            let who = person(ctx, &p["responsible"]).ok().or(ctx.row.assigned_to);
            if let Some(u) = who {
                m.insert("assigned_to".into(), json!(u));
            }
            if let Some(c) = p["category_id"].as_str().filter(|s| !s.is_empty()) {
                m.insert("category_id".into(), json!(c));
            }
            if p["copy_client"].as_bool() == Some(true) && (tt.has_client || !tt.is_custom()) {
                let company = if et.id == COMPANY { Some(item) } else { ctx.row.company_id };
                let contacts: Vec<Uuid> = if et.id == CONTACT {
                    vec![item]
                } else {
                    sqlx::query_scalar("SELECT contact_id FROM crm_item_contacts WHERE item_id = $1 ORDER BY is_primary DESC, sort")
                        .bind(item)
                        .fetch_all(&state.db)
                        .await
                        .map_err(err)?
                };
                if let Some(c) = company {
                    m.insert("company_id".into(), json!(c));
                }
                if !contacts.is_empty() {
                    m.insert("contact_ids".into(), json!(contacts));
                }
            }
            if p["copy_amount"].as_bool() == Some(true) && tt.has_amount && et.has_amount {
                m.insert("opportunity".into(), json!(ctx.row.opportunity.to_string()));
                m.insert("currency".into(), json!(ctx.row.currency));
                m.insert("is_manual_amount".into(), json!(true));
            }
            let id = create_item(state, &system_auth(), &tt, m, Some(item)).await.map_err(err)?;
            let number: i64 = sqlx::query_scalar("SELECT number FROM crm_items WHERE id = $1").bind(id).fetch_one(&state.db).await.map_err(err)?;
            timeline(&state.db, item, "linked", Some(SYSTEM_USER), None, json!({ "item_id": id, "entity_type": tt.id, "code": tt.code, "number": number, "title": f("title") })).await.map_err(err)?;
            done(format!("created {} #{number}", tt.name.to_lowercase()))
        }
        "observers" => {
            let mut add = Vec::new();
            for v in p["users"].as_array().cloned().unwrap_or_default() {
                if let Ok(u) = person(ctx, &v)
                    && active(state, u).await? && !add.contains(&u) {
                        add.push(u);
                    }
            }
            let n = sqlx::query("INSERT INTO crm_item_observers (item_id, user_id, added_by) SELECT $1, UNNEST($2::uuid[]), $3 ON CONFLICT DO NOTHING")
                .bind(item)
                .bind(&add)
                .bind(SYSTEM_USER)
                .execute(&state.db)
                .await
                .map_err(err)?
                .rows_affected();
            if n > 0 {
                broadcast(state, et.id, item, false).await;
            }
            done(format!("added {n} observer{}", if n == 1 { "" } else { "s" }))
        }
        "convert_lead" => {
            if et.id != LEAD {
                return Err("only leads can be converted".into());
            }
            if ctx.row.stage_semantics == "success" {
                return Err("the lead has already been converted".into());
            }
            let body = Convert {
                create_contact: p["create_contact"].as_bool() == Some(true),
                create_company: p["create_company"].as_bool() == Some(true),
                create_deal: p["create_deal"].as_bool() == Some(true),
                deal_category_id: p["deal_category_id"].as_str().and_then(|s| Uuid::parse_str(s).ok()),
                ..Default::default()
            };
            let r = crate::crm::items::convert_lead(state, &system_auth(), item, body).await.map_err(err)?;
            let made: Vec<&str> = [("contact_id", "a contact"), ("company_id", "a company"), ("deal_id", "a deal")].iter().filter(|(k, _)| !r[*k].is_null()).map(|(_, l)| *l).collect();
            done(format!("converted the lead into {}", made.join(", ")))
        }
        other => Err(format!("unknown action {other}").into()),
    }
}

// ---------------------------------------------------------------------------
// Outgoing webhooks
// ---------------------------------------------------------------------------

async fn call_webhook(state: &AppState, robot: &Robot, ctx: &Ctx) -> Result<String, Fail> {
    let p = &robot.params;
    let url = check_url(&fill(p["url"].as_str().unwrap_or(""), &ctx.value))?;
    let method = reqwest::Method::from_bytes(p["method"].as_str().unwrap_or("POST").as_bytes()).map_err(err)?;
    let template = p["body"].as_str().unwrap_or("");
    let (body, content_type) = if template.trim().is_empty() {
        let record = json!({ "event": "robot", "robot": robot.name, "record": ctx.value });
        (record.to_string(), "application/json")
    } else if template.trim_start().starts_with(['{', '[']) {
        (fill_json(template, &ctx.value), "application/json")
    } else {
        (fill(template, &ctx.value), "text/plain; charset=utf-8")
    };
    let has_body = !matches!(method, reqwest::Method::GET | reqwest::Method::DELETE);
    let mut headers = vec![("X-NERP-Event".to_string(), "robot".to_string()), ("X-NERP-Record".to_string(), ctx.row.id.to_string())];
    for h in p["headers"].as_array().into_iter().flatten() {
        headers.push((h["name"].as_str().unwrap_or("").to_string(), fill(h["value"].as_str().unwrap_or(""), &ctx.value)));
    }
    let r = crate::outbound::send(state, crate::outbound::Request {
        method,
        url,
        headers,
        body: has_body.then_some((body, content_type)),
        secret: p["secret"].as_str(),
        timeout_secs: 15,
    })
    .await
    .map_err(|f| Fail { msg: f.msg, retry: f.retry })?;
    Ok(r.summary)
}
