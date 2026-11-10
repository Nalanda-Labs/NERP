//! Business processes on list items.
//!
//! A process is a tree of steps, run in order:
//! - `approval`: people approve or reject (any one of them, or all); then the `approved` or
//!   `rejected` steps run. A rejection with no steps of its own ends the process as rejected.
//! - `condition`: compares a field (equals, differs, more, less, empty, filled) and runs the
//!   `then` or `else` steps.
//! - `notify`: a notification to people. `task`: a task for someone. `set_field`: changes a
//!   field of the item. `wait`: pauses for some hours. `stop`: ends the process.
//!
//! People are chosen as `{"users": [...], "author": true, "head": true}`: given employees, the
//! item's author, and the author's nearest department head (HR managers when there is none).
//! Texts may use {{title}}, {{number}}, {{author}} and {{<field code>}}.
//!
//! A process keeps a copy of its steps from when it started, so editing the template doesn't
//! change processes already running. Its position is a stack of frames, each naming a step
//! list in the tree and the next step in it.

use anyhow::Result;
use axum::{
    extract::{Path, State},
    routing::{get, patch, post},
    Json, Router,
};
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{
    auth::AuthUser,
    crm::automation::system_auth,
    error::{AppError, AppResult},
    lists::{fields, item_row, need, Field},
    notify::{notify, NewNotification},
    routes::required,
    state::AppState,
};

pub const RESUME_JOB: &str = "wf.resume";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/lists/{id}/workflows", get(list_templates).post(create_template))
        .route("/workflows/{id}", get(get_template).patch(update_template).delete(delete_template))
        .route("/lists/items/{id}/start/{template}", post(start_manual))
        .route("/workflows/instances/{id}/cancel", post(cancel))
        .route("/approvals", get(my_approvals))
        .route("/approvals/{id}", patch(decide))
}

// ---------------------------------------------------------------------------
// Steps
// ---------------------------------------------------------------------------

const STEP_KINDS: &[&str] = &["approval", "condition", "notify", "task", "set_field", "wait", "stop"];
const OPS: &[&str] = &["eq", "ne", "gt", "lt", "empty", "filled"];

fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or("").trim().to_string()
}

fn people_ok(v: &Value) -> AppResult<Value> {
    let users: Vec<Uuid> = v.get("users").and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().and_then(|s| Uuid::parse_str(s).ok())).collect()).unwrap_or_default();
    let author = v.get("author").and_then(Value::as_bool).unwrap_or(false);
    let head = v.get("head").and_then(Value::as_bool).unwrap_or(false);
    if users.is_empty() && !author && !head {
        return Err(AppError::bad("choose who: people, the author, or the author's head"));
    }
    Ok(json!({ "users": users, "author": author, "head": head }))
}

/// Checks a step list (recursively) against the list's fields; keeps only known keys.
fn clean_steps(v: &Value, fs: &[Field], depth: usize, count: &mut usize) -> AppResult<Value> {
    let list = v.as_array().ok_or_else(|| AppError::bad("steps must be a list"))?;
    if depth > 6 {
        return Err(AppError::bad("steps can be nested at most 6 deep"));
    }
    let mut out = Vec::new();
    for st in list {
        *count += 1;
        if *count > 100 {
            return Err(AppError::bad("at most 100 steps"));
        }
        let kind = s(st, "kind");
        if !STEP_KINDS.contains(&kind.as_str()) {
            return Err(AppError::bad("unknown step"));
        }
        let field = |code: &str| fs.iter().find(|f| f.code == code).ok_or_else(|| AppError::bad(format!("unknown field “{code}”")));
        let mut o = json!({ "kind": kind });
        match kind.as_str() {
            "approval" => {
                o["title"] = json!(required("approval title", &s(st, "title"), 200)?);
                o["who"] = people_ok(st.get("who").unwrap_or(&Value::Null))?;
                o["mode"] = json!(if s(st, "mode") == "all" { "all" } else { "any" });
                if let Some(h) = st.get("due_hours").and_then(Value::as_i64).filter(|h| *h > 0) {
                    o["due_hours"] = json!(h.min(24 * 90));
                }
                o["approved"] = clean_steps(st.get("approved").unwrap_or(&json!([])), fs, depth + 1, count)?;
                o["rejected"] = clean_steps(st.get("rejected").unwrap_or(&json!([])), fs, depth + 1, count)?;
            }
            "condition" => {
                let f = field(&s(st, "field"))?;
                let op = s(st, "op");
                if !OPS.contains(&op.as_str()) {
                    return Err(AppError::bad("unknown comparison"));
                }
                o["field"] = json!(f.code);
                o["op"] = json!(op);
                o["value"] = json!(s(st, "value"));
                o["then"] = clean_steps(st.get("then").unwrap_or(&json!([])), fs, depth + 1, count)?;
                o["else"] = clean_steps(st.get("else").unwrap_or(&json!([])), fs, depth + 1, count)?;
            }
            "notify" => {
                o["who"] = people_ok(st.get("who").unwrap_or(&Value::Null))?;
                o["text"] = json!(required("notification text", &s(st, "text"), 500)?);
            }
            "task" => {
                o["who"] = people_ok(st.get("who").unwrap_or(&Value::Null))?;
                o["title"] = json!(required("task title", &s(st, "title"), 300)?);
                o["description"] = json!(s(st, "description").chars().take(5000).collect::<String>());
                if let Some(h) = st.get("deadline_hours").and_then(Value::as_i64).filter(|h| *h > 0) {
                    o["deadline_hours"] = json!(h.min(24 * 365));
                }
            }
            "set_field" => {
                let f = field(&s(st, "field"))?;
                if ["file", "user", "crm"].contains(&f.kind.as_str()) {
                    return Err(AppError::bad(format!("“{}” can't be set by a process", f.label)));
                }
                if f.kind == "select" && !f.options.contains(&s(st, "value")) && !s(st, "value").is_empty() {
                    return Err(AppError::bad(format!("“{}” is not a choice of “{}”", s(st, "value"), f.label)));
                }
                o["field"] = json!(f.code);
                o["value"] = json!(s(st, "value"));
            }
            "wait" => {
                let h = st.get("hours").and_then(Value::as_f64).filter(|h| *h > 0.0 && *h <= 24.0 * 90.0).ok_or_else(|| AppError::bad("wait between a minute and 90 days"))?;
                o["hours"] = json!(h);
            }
            _ => o["status"] = json!(if s(st, "status") == "rejected" { "rejected" } else { "completed" }),
        }
        out.push(o);
    }
    Ok(Value::Array(out))
}

/// A frame: a step list (named by the branches taken to reach it) and the next step in it.
#[derive(Serialize, Deserialize, Clone)]
struct Frame {
    at: Vec<(usize, String)>,
    idx: usize,
}

fn resolve<'a>(root: &'a Value, at: &[(usize, String)]) -> Option<&'a Vec<Value>> {
    let mut list = root.as_array()?;
    for (i, branch) in at {
        list = list.get(*i)?.get(branch)?.as_array()?;
    }
    Some(list)
}

// ---------------------------------------------------------------------------
// Templates
// ---------------------------------------------------------------------------

async fn list_templates(State(state): State<AppState>, auth: AuthUser, Path(list): Path<Uuid>) -> AppResult<Json<Value>> {
    need(&state, &auth, list, "read").await?;
    let rows: Vec<(Uuid, String, String, Value, bool)> = sqlx::query_as("SELECT id, name, trigger, steps, is_active FROM wf_templates WHERE list_id = $1 ORDER BY name").bind(list).fetch_all(&state.db).await?;
    Ok(Json(json!({ "items": rows.into_iter().map(|(id, n, t, st, a)| json!({ "id": id, "name": n, "trigger": t, "steps": st, "is_active": a })).collect::<Vec<_>>() })))
}

#[derive(Deserialize)]
struct TemplateInput {
    name: Option<String>,
    trigger: Option<String>,
    steps: Option<Value>,
    is_active: Option<bool>,
}

async fn create_template(State(state): State<AppState>, auth: AuthUser, Path(list): Path<Uuid>, Json(b): Json<TemplateInput>) -> AppResult<Json<Value>> {
    need(&state, &auth, list, "manage").await?;
    let name = required("name", b.name.as_deref().unwrap_or(""), 120)?;
    let trigger = b.trigger.unwrap_or_else(|| "manual".into());
    if !["create", "update", "manual"].contains(&trigger.as_str()) {
        return Err(AppError::bad("start on create, update or by hand"));
    }
    let fs = fields(&state, list).await?;
    let steps = clean_steps(&b.steps.unwrap_or(json!([])), &fs, 0, &mut 0)?;
    let id: Uuid = sqlx::query_scalar("INSERT INTO wf_templates (list_id, name, trigger, steps, is_active, created_by) VALUES ($1, $2, $3, $4, $5, $6) RETURNING id")
        .bind(list)
        .bind(&name)
        .bind(&trigger)
        .bind(&steps)
        .bind(b.is_active.unwrap_or(true))
        .bind(auth.id)
        .fetch_one(&state.db)
        .await?;
    get_template(State(state), auth, Path(id)).await
}

async fn template_list(state: &AppState, id: Uuid) -> AppResult<Uuid> {
    sqlx::query_scalar("SELECT list_id FROM wf_templates WHERE id = $1").bind(id).fetch_optional(&state.db).await?.ok_or(AppError::NotFound("process"))
}

async fn get_template(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let list = template_list(&state, id).await?;
    need(&state, &auth, list, "read").await?;
    let (name, trigger, steps, active): (String, String, Value, bool) = sqlx::query_as("SELECT name, trigger, steps, is_active FROM wf_templates WHERE id = $1").bind(id).fetch_one(&state.db).await?;
    Ok(Json(json!({ "id": id, "list_id": list, "name": name, "trigger": trigger, "steps": steps, "is_active": active })))
}

async fn update_template(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<TemplateInput>) -> AppResult<Json<Value>> {
    let list = template_list(&state, id).await?;
    need(&state, &auth, list, "manage").await?;
    let name = b.name.as_deref().map(|n| required("name", n, 120)).transpose()?;
    if b.trigger.as_ref().is_some_and(|t| !["create", "update", "manual"].contains(&t.as_str())) {
        return Err(AppError::bad("start on create, update or by hand"));
    }
    let steps = match &b.steps {
        Some(v) => Some(clean_steps(v, &fields(&state, list).await?, 0, &mut 0)?),
        None => None,
    };
    sqlx::query("UPDATE wf_templates SET name = COALESCE($2, name), trigger = COALESCE($3, trigger), steps = COALESCE($4, steps), is_active = COALESCE($5, is_active), updated_at = now() WHERE id = $1")
        .bind(id)
        .bind(name)
        .bind(b.trigger)
        .bind(steps)
        .bind(b.is_active)
        .execute(&state.db)
        .await?;
    get_template(State(state), auth, Path(id)).await
}

/// Processes already running carry on with their own copy of the steps.
async fn delete_template(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let list = template_list(&state, id).await?;
    need(&state, &auth, list, "manage").await?;
    sqlx::query("DELETE FROM wf_templates WHERE id = $1").bind(id).execute(&state.db).await?;
    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// Running
// ---------------------------------------------------------------------------

/// Starts the processes a list item's creation or change calls for. Errors are logged.
pub async fn on_item(state: &AppState, auth: &AuthUser, list: Uuid, item: Uuid, trigger: &str) {
    let templates: Vec<Uuid> = sqlx::query_scalar(
        "SELECT t.id FROM wf_templates t WHERE t.list_id = $1 AND t.trigger = $2 AND t.is_active
           AND NOT EXISTS (SELECT 1 FROM wf_instances w WHERE w.template_id = t.id AND w.item_id = $3 AND w.status IN ('running', 'waiting'))",
    )
    .bind(list)
    .bind(trigger)
    .bind(item)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();
    for t in templates {
        if let Err(e) = start(state, t, item, Some(auth.id)).await {
            tracing::warn!(template = %t, item = %item, error = ?e, "business process could not start");
        }
    }
}

async fn start(state: &AppState, template: Uuid, item: Uuid, by: Option<Uuid>) -> AppResult<Uuid> {
    let (name, steps): (String, Value) = sqlx::query_as("SELECT name, steps FROM wf_templates WHERE id = $1").bind(template).fetch_one(&state.db).await?;
    let cursor = json!([Frame { at: vec![], idx: 0 }]);
    let id: Uuid = sqlx::query_scalar("INSERT INTO wf_instances (template_id, name, item_id, steps, cursor, started_by) VALUES ($1, $2, $3, $4, $5, $6) RETURNING id")
        .bind(template)
        .bind(&name)
        .bind(item)
        .bind(&steps)
        .bind(cursor)
        .bind(by)
        .fetch_one(&state.db)
        .await?;
    log(state, id, by, "Started".into()).await;
    run(state, id).await?;
    Ok(id)
}

async fn start_manual(State(state): State<AppState>, auth: AuthUser, Path((item, template)): Path<(Uuid, Uuid)>) -> AppResult<Json<Value>> {
    let (list, author) = item_row(&state, item).await?;
    let l = need(&state, &auth, list, "add").await?;
    if l != "manage" && author != Some(auth.id) {
        return Err(AppError::Forbidden);
    }
    let ok: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM wf_templates WHERE id = $1 AND list_id = $2 AND is_active)").bind(template).bind(list).fetch_one(&state.db).await?;
    if !ok {
        return Err(AppError::NotFound("process"));
    }
    let running: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM wf_instances WHERE template_id = $1 AND item_id = $2 AND status IN ('running', 'waiting'))").bind(template).bind(item).fetch_one(&state.db).await?;
    if running {
        return Err(AppError::bad("this process is already running on the item"));
    }
    start(&state, template, item, Some(auth.id)).await?;
    Ok(Json(item_processes(&state, item).await?))
}

async fn log(state: &AppState, instance: Uuid, user: Option<Uuid>, text: String) {
    let _ = sqlx::query("INSERT INTO wf_log (instance_id, user_id, text) VALUES ($1, $2, $3)").bind(instance).bind(user).bind(text).execute(&state.db).await;
}

struct Item {
    id: Uuid,
    list_id: Uuid,
    number: i64,
    title: String,
    values: Value,
    author: Option<Uuid>,
}

async fn item(state: &AppState, id: Uuid) -> AppResult<Item> {
    let (list_id, number, title, values, author): (Uuid, i64, String, Value, Option<Uuid>) =
        sqlx::query_as("SELECT list_id, number, title, \"values\", created_by FROM list_items WHERE id = $1").bind(id).fetch_one(&state.db).await?;
    Ok(Item { id, list_id, number, title, values, author })
}

/// Fills {{title}}, {{number}}, {{author}} and field codes.
async fn fill(state: &AppState, text: &str, it: &Item) -> String {
    let mut out = text.replace("{{title}}", &it.title).replace("{{number}}", &it.number.to_string());
    if out.contains("{{author}}") {
        let name: String = match it.author {
            Some(a) => sqlx::query_scalar("SELECT trim(first_name || ' ' || last_name) FROM users WHERE id = $1").bind(a).fetch_one(&state.db).await.unwrap_or_default(),
            None => String::new(),
        };
        out = out.replace("{{author}}", &name);
    }
    if let Some(m) = it.values.as_object() {
        for (k, v) in m {
            out = out.replace(&format!("{{{{{k}}}}}"), &v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string()));
        }
    }
    out
}

async fn people(state: &AppState, who: &Value, it: &Item) -> AppResult<Vec<Uuid>> {
    let mut out: Vec<Uuid> = who.get("users").and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().and_then(|s| Uuid::parse_str(s).ok())).collect()).unwrap_or_default();
    if who.get("author").and_then(Value::as_bool).unwrap_or(false) {
        out.extend(it.author);
    }
    if who.get("head").and_then(Value::as_bool).unwrap_or(false)
        && let Some(a) = it.author {
            out.extend(crate::hr::approvers(state, a).await?);
        }
    let active: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM users WHERE id = ANY($1) AND is_active").bind(&out).fetch_all(&state.db).await?;
    out.retain(|u| active.contains(u));
    out.sort();
    out.dedup();
    Ok(out)
}

fn compare(fs: &[Field], values: &Value, field: &str, op: &str, want: &str) -> bool {
    let v = values.get(field);
    let text = v.map(|v| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string())).unwrap_or_default();
    let empty = v.is_none() || v.is_some_and(Value::is_null) || text.is_empty() || v == Some(&json!(false));
    let numeric = fs.iter().find(|f| f.code == field).is_some_and(|f| ["number", "money"].contains(&f.kind.as_str()));
    let num = |x: &str| x.replace(',', ".").parse::<f64>().ok();
    match op {
        "empty" => empty,
        "filled" => !empty,
        "gt" | "lt" => match (num(&text), num(want)) {
            (Some(a), Some(b)) => if op == "gt" { a > b } else { a < b },
            _ => if op == "gt" { text.as_str() > want } else { text.as_str() < want },
        },
        "eq" => if numeric { num(&text) == num(want) } else { text.eq_ignore_ascii_case(want) },
        _ => if numeric { num(&text) != num(want) } else { !text.eq_ignore_ascii_case(want) },
    }
}

async fn finish(state: &AppState, id: Uuid, status: &str, text: String) -> AppResult<()> {
    sqlx::query("UPDATE wf_instances SET status = $2, finished_at = now(), cursor = '[]' WHERE id = $1").bind(id).bind(status).execute(&state.db).await?;
    sqlx::query("UPDATE wf_approvals SET status = 'cancelled' WHERE instance_id = $1 AND status = 'pending'").bind(id).execute(&state.db).await?;
    log(state, id, None, text).await;
    // The author hears how it ended.
    let (item_id, name): (Uuid, String) = sqlx::query_as("SELECT item_id, name FROM wf_instances WHERE id = $1").bind(id).fetch_one(&state.db).await?;
    let it = item(state, item_id).await?;
    if let Some(a) = it.author {
        let how = match status { "completed" => "is complete", "rejected" => "was rejected", "cancelled" => "was cancelled", _ => "stopped with an error" };
        notify(state, &[a], NewNotification { kind: "workflow.done", title: format!("“{name}” {how}: {}", it.title), body: None, link: Some(format!("/lists/{}?item={}", it.list_id, it.id)), actor: None }).await;
    }
    Ok(())
}

/// Runs the process from where it is until it waits or ends.
pub async fn run(state: &AppState, id: Uuid) -> AppResult<()> {
    let (status, steps, cursor, item_id): (String, Value, Value, Uuid) = sqlx::query_as("SELECT status, steps, cursor, item_id FROM wf_instances WHERE id = $1").bind(id).fetch_one(&state.db).await?;
    if !["running", "waiting"].contains(&status.as_str()) {
        return Ok(());
    }
    let mut stack: Vec<Frame> = serde_json::from_value(cursor).unwrap_or_default();
    let mut it = item(state, item_id).await?;
    let fs = fields(state, it.list_id).await?;
    sqlx::query("UPDATE wf_instances SET status = 'running' WHERE id = $1").bind(id).execute(&state.db).await?;
    for _ in 0..500 {
        let Some(frame) = stack.last().cloned() else {
            return finish(state, id, "completed", "Completed".into()).await;
        };
        let Some(list) = resolve(&steps, &frame.at) else {
            sqlx::query("UPDATE wf_instances SET status = 'failed', error = 'its steps are damaged', finished_at = now() WHERE id = $1").bind(id).execute(&state.db).await?;
            return Ok(());
        };
        if frame.idx >= list.len() {
            stack.pop();
            continue;
        }
        let step = list[frame.idx].clone();
        stack.last_mut().unwrap().idx += 1;
        let here = |branch: &str| {
            let mut at = frame.at.clone();
            at.push((frame.idx, branch.to_string()));
            Frame { at, idx: 0 }
        };
        match s(&step, "kind").as_str() {
            "condition" => {
                let yes = compare(&fs, &it.values, &s(&step, "field"), &s(&step, "op"), &s(&step, "value"));
                log(state, id, None, format!("Condition on “{}”: {}", s(&step, "field"), if yes { "yes" } else { "no" })).await;
                stack.push(here(if yes { "then" } else { "else" }));
            }
            "notify" => {
                let who = people(state, &step["who"], &it).await?;
                let text = fill(state, &s(&step, "text"), &it).await;
                notify(state, &who, NewNotification { kind: "workflow.notify", title: text.clone(), body: Some(it.title.clone()), link: Some(format!("/lists/{}?item={}", it.list_id, it.id)), actor: None }).await;
                log(state, id, None, format!("Notified {} {}: {text}", who.len(), if who.len() == 1 { "person" } else { "people" })).await;
            }
            "task" => {
                let who = people(state, &step["who"], &it).await?;
                let deadline = step.get("deadline_hours").and_then(Value::as_i64).map(|h| Utc::now() + Duration::hours(h));
                let title = fill(state, &s(&step, "title"), &it).await;
                let description = fill(state, &s(&step, "description"), &it).await;
                let process: String = sqlx::query_scalar("SELECT name FROM wf_instances WHERE id = $1").bind(id).fetch_one(&state.db).await?;
                for u in &who {
                    let body: crate::tasks::items::CreateTask = serde_json::from_value(json!({
                        "title": title, "description": format!("{description}\n\nFrom the process “{process}” on {} #{}.", it.title, it.number),
                        "responsible_id": u, "deadline": deadline,
                    }))
                    .map_err(|e| AppError::Internal(e.into()))?;
                    crate::tasks::items::create_task(state, &system_auth(), body).await?;
                }
                log(state, id, None, format!("Task “{title}” for {} {}", who.len(), if who.len() == 1 { "person" } else { "people" })).await;
            }
            "set_field" => {
                let field = s(&step, "field");
                let value = s(&step, "value");
                let f = fs.iter().find(|f| f.code == field);
                let v = match f.map(|f| f.kind.as_str()) {
                    _ if value.is_empty() => Value::Null,
                    Some("number") => value.replace(',', ".").parse::<f64>().map(|n| json!(n)).unwrap_or(Value::Null),
                    Some("checkbox") => json!(value == "true" || value == "yes" || value == "1"),
                    _ => json!(fill(state, &value, &it).await),
                };
                let mut vals = it.values.as_object().cloned().unwrap_or_default();
                if v.is_null() { vals.remove(&field); } else { vals.insert(field.clone(), v); }
                it.values = Value::Object(vals);
                sqlx::query("UPDATE list_items SET \"values\" = $2, updated_at = now() WHERE id = $1").bind(it.id).bind(&it.values).execute(&state.db).await?;
                log(state, id, None, format!("Set “{}” to “{value}”", f.map(|f| f.label.as_str()).unwrap_or(&field))).await;
            }
            "wait" => {
                let hours = step["hours"].as_f64().unwrap_or(1.0);
                let at = Utc::now() + Duration::seconds((hours * 3600.0) as i64);
                save(state, id, &stack, "waiting").await?;
                crate::jobs::enqueue(&state.db, RESUME_JOB, json!({ "instance": id }), Some(at)).await?;
                log(state, id, None, format!("Waiting until {}", at.format("%Y-%m-%d %H:%M UTC"))).await;
                return Ok(());
            }
            "approval" => {
                let who = people(state, &step["who"], &it).await?;
                if who.is_empty() {
                    sqlx::query("UPDATE wf_instances SET status = 'failed', error = 'nobody to approve', finished_at = now() WHERE id = $1").bind(id).execute(&state.db).await?;
                    log(state, id, None, "Stopped: nobody to approve".into()).await;
                    return Ok(());
                }
                let title = fill(state, &s(&step, "title"), &it).await;
                let due = step.get("due_hours").and_then(Value::as_i64).map(|h| Utc::now() + Duration::hours(h));
                let at = json!({ "at": frame.at, "idx": frame.idx });
                sqlx::query("INSERT INTO wf_approvals (instance_id, title, approvers, mode, at, due_at) VALUES ($1, $2, $3, $4, $5, $6)")
                    .bind(id)
                    .bind(&title)
                    .bind(&who)
                    .bind(s(&step, "mode"))
                    .bind(at)
                    .bind(due)
                    .execute(&state.db)
                    .await?;
                save(state, id, &stack, "waiting").await?;
                notify(state, &who, NewNotification { kind: "workflow.approval", title: format!("Please approve: {title}"), body: Some(it.title.clone()), link: Some("/approvals".into()), actor: None }).await;
                crate::realtime::publish(state, Some(who.clone()), "approvals.changed", json!({})).await;
                log(state, id, None, format!("Waiting for approval: {title}")).await;
                return Ok(());
            }
            _ => {
                let st = if s(&step, "status") == "rejected" { "rejected" } else { "completed" };
                return finish(state, id, st, format!("Stopped: {st}")).await;
            }
        }
    }
    sqlx::query("UPDATE wf_instances SET status = 'failed', error = 'too many steps', finished_at = now() WHERE id = $1").bind(id).execute(&state.db).await?;
    Ok(())
}

async fn save(state: &AppState, id: Uuid, stack: &[Frame], status: &str) -> AppResult<()> {
    sqlx::query("UPDATE wf_instances SET cursor = $2, status = $3 WHERE id = $1").bind(id).bind(json!(stack)).bind(status).execute(&state.db).await?;
    Ok(())
}

/// The job that wakes a waiting process.
pub async fn resume(state: &AppState, payload: &Value) -> Result<()> {
    let id = payload["instance"].as_str().and_then(|s| Uuid::parse_str(s).ok()).ok_or_else(|| anyhow::anyhow!("bad payload"))?;
    let waiting_on_approval: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM wf_approvals WHERE instance_id = $1 AND status = 'pending')").bind(id).fetch_one(&state.db).await?;
    if !waiting_on_approval {
        run(state, id).await.map_err(|e| anyhow::anyhow!("{e:?}"))?;
    }
    Ok(())
}

async fn cancel(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let (item_id, started_by, status): (Uuid, Option<Uuid>, String) = sqlx::query_as("SELECT item_id, started_by, status FROM wf_instances WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound("process"))?;
    let (list, _) = item_row(&state, item_id).await?;
    let l = need(&state, &auth, list, "read").await?;
    if l != "manage" && started_by != Some(auth.id) {
        return Err(AppError::Forbidden);
    }
    if !["running", "waiting"].contains(&status.as_str()) {
        return Err(AppError::bad("this process has already ended"));
    }
    let name: String = sqlx::query_scalar("SELECT trim(first_name || ' ' || last_name) FROM users WHERE id = $1").bind(auth.id).fetch_one(&state.db).await?;
    finish(&state, id, "cancelled", format!("Cancelled by {name}")).await?;
    Ok(Json(item_processes(&state, item_id).await?))
}

/// An item's processes, newest first, with their log and approvals.
pub async fn item_processes(state: &AppState, item: Uuid) -> AppResult<Value> {
    let rows: Vec<(Uuid, String, String, chrono::DateTime<Utc>, Option<chrono::DateTime<Utc>>, Option<String>)> =
        sqlx::query_as("SELECT id, name, status, started_at, finished_at, error FROM wf_instances WHERE item_id = $1 ORDER BY started_at DESC").bind(item).fetch_all(&state.db).await?;
    let mut out = Vec::new();
    for (id, name, status, started, finished, error) in rows {
        let log: Vec<(String, Option<String>, chrono::DateTime<Utc>)> = sqlx::query_as(
            "SELECT l.text, trim(u.first_name || ' ' || u.last_name), l.at FROM wf_log l LEFT JOIN users u ON u.id = l.user_id WHERE l.instance_id = $1 ORDER BY l.at",
        )
        .bind(id)
        .fetch_all(&state.db)
        .await?;
        let approvals: Vec<(Uuid, String, String, Value)> = sqlx::query_as("SELECT id, title, status, decisions FROM wf_approvals WHERE instance_id = $1 ORDER BY created_at").bind(id).fetch_all(&state.db).await?;
        out.push(json!({
            "id": id, "name": name, "status": status, "started_at": started, "finished_at": finished, "error": error,
            "log": log.into_iter().map(|(t, u, at)| json!({ "text": t, "user": u, "at": at })).collect::<Vec<_>>(),
            "approvals": approvals.into_iter().map(|(id, t, st, d)| json!({ "id": id, "title": t, "status": st, "decisions": d })).collect::<Vec<_>>(),
        }));
    }
    Ok(json!(out))
}

// ---------------------------------------------------------------------------
// Approvals
// ---------------------------------------------------------------------------

async fn my_approvals(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    let rows: Vec<(Uuid, String, String, Option<chrono::DateTime<Utc>>, chrono::DateTime<Utc>, Uuid, String, i64, Uuid, String, Option<String>, Value)> = sqlx::query_as(
        "SELECT a.id, a.title, a.mode, a.due_at, a.created_at, i.id, i.title, i.number, l.id, l.name, trim(u.first_name || ' ' || u.last_name), a.decisions
         FROM wf_approvals a JOIN wf_instances w ON w.id = a.instance_id JOIN list_items i ON i.id = w.item_id JOIN lists l ON l.id = i.list_id
         LEFT JOIN users u ON u.id = i.created_by
         WHERE a.status = 'pending' AND $1 = ANY(a.approvers) AND NOT a.decisions @> jsonb_build_array(jsonb_build_object('user_id', $1::text))
         ORDER BY a.created_at",
    )
    .bind(auth.id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({ "items": rows.into_iter().map(|(id, title, mode, due, created, item_id, item_title, number, list_id, list_name, author, decisions)| json!({
        "id": id, "title": title, "mode": mode, "due_at": due, "created_at": created, "item_id": item_id, "item_title": item_title, "number": number,
        "list_id": list_id, "list_name": list_name, "author": author, "decisions": decisions,
    })).collect::<Vec<_>>() })))
}

#[derive(Deserialize)]
struct Decision {
    approve: bool,
    comment: Option<String>,
}

async fn decide(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<Decision>) -> AppResult<Json<Value>> {
    let mut tx = state.db.begin().await?;
    let (instance, approvers, mode, status, decisions, at): (Uuid, Vec<Uuid>, String, String, Value, Value) =
        sqlx::query_as("SELECT instance_id, approvers, mode, status, decisions, at FROM wf_approvals WHERE id = $1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(AppError::NotFound("approval"))?;
    if !approvers.contains(&auth.id) {
        return Err(AppError::NotFound("approval"));
    }
    if status != "pending" {
        return Err(AppError::bad("this has already been decided"));
    }
    let mut list: Vec<Value> = decisions.as_array().cloned().unwrap_or_default();
    if list.iter().any(|d| d["user_id"] == json!(auth.id.to_string())) {
        return Err(AppError::bad("you have already decided"));
    }
    let comment = b.comment.map(|c| c.trim().chars().take(1000).collect::<String>()).filter(|c| !c.is_empty());
    list.push(json!({ "user_id": auth.id.to_string(), "decision": if b.approve { "approved" } else { "rejected" }, "comment": comment, "at": Utc::now() }));
    let approvals = list.iter().filter(|d| d["decision"] == "approved").count();
    let settled = if !b.approve {
        Some("rejected")
    } else if mode == "any" || approvals == approvers.len() {
        Some("approved")
    } else {
        None
    };
    sqlx::query("UPDATE wf_approvals SET decisions = $2, status = COALESCE($3, status), decided_at = CASE WHEN $3 IS NULL THEN NULL ELSE now() END WHERE id = $1")
        .bind(id)
        .bind(json!(list))
        .bind(settled)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    let me: String = sqlx::query_scalar("SELECT trim(first_name || ' ' || last_name) FROM users WHERE id = $1").bind(auth.id).fetch_one(&state.db).await?;
    log(&state, instance, Some(auth.id), format!("{me} {}{}", if b.approve { "approved" } else { "rejected" }, comment.map(|c| format!(": “{c}”")).unwrap_or_default())).await;
    crate::realtime::publish(&state, Some(approvers.clone()), "approvals.changed", json!({})).await;
    if let Some(outcome) = settled {
        // Go on with the branch for this outcome, after the approval step.
        let (steps, cursor): (Value, Value) = sqlx::query_as("SELECT steps, cursor FROM wf_instances WHERE id = $1").bind(instance).fetch_one(&state.db).await?;
        let mut stack: Vec<Frame> = serde_json::from_value(cursor).unwrap_or_default();
        let at_path: Vec<(usize, String)> = serde_json::from_value(at["at"].clone()).unwrap_or_default();
        let idx = at["idx"].as_u64().unwrap_or(0) as usize;
        let branch_steps = resolve(&steps, &at_path).and_then(|l| l.get(idx)).and_then(|st| st.get(outcome)).and_then(Value::as_array).map(|a| a.len()).unwrap_or(0);
        if outcome == "rejected" && branch_steps == 0 {
            finish(&state, instance, "rejected", "Rejected".into()).await?;
        } else {
            let mut p = at_path.clone();
            p.push((idx, outcome.to_string()));
            stack.push(Frame { at: p, idx: 0 });
            save(&state, instance, &stack, "running").await?;
            run(&state, instance).await?;
        }
    }
    my_approvals(State(state), auth).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(code: &str, kind: &str) -> Field {
        Field { id: None, code: code.into(), label: code.into(), kind: kind.into(), options: vec!["A".into(), "B".into()], required: false }
    }

    #[test]
    fn steps_are_checked_and_trimmed() {
        let fs = vec![f("amount", "number"), f("kind", "select")];
        let v = clean_steps(&json!([
            { "kind": "condition", "field": "amount", "op": "gt", "value": "1000", "junk": 1,
              "then": [{ "kind": "approval", "title": "Boss", "who": { "head": true }, "rejected": [{ "kind": "stop", "status": "rejected" }] }],
              "else": [{ "kind": "set_field", "field": "kind", "value": "A" }] }
        ]), &fs, 0, &mut 0).unwrap();
        assert!(v[0].get("junk").is_none());
        assert_eq!(v[0]["then"][0]["mode"], "any");
        assert!(clean_steps(&json!([{ "kind": "set_field", "field": "kind", "value": "Z" }]), &fs, 0, &mut 0).is_err());
        assert!(clean_steps(&json!([{ "kind": "approval", "title": "x", "who": {} }]), &fs, 0, &mut 0).is_err());
        assert!(clean_steps(&json!([{ "kind": "condition", "field": "nope", "op": "eq" }]), &fs, 0, &mut 0).is_err());
    }

    #[test]
    fn walking_the_tree() {
        let steps = json!([{ "kind": "condition", "then": [{ "kind": "notify" }, { "kind": "stop" }] }]);
        assert_eq!(resolve(&steps, &[(0, "then".into())]).unwrap().len(), 2);
        assert!(resolve(&steps, &[(0, "else".into())]).is_none());
    }

    #[test]
    fn comparisons() {
        let fs = vec![f("amount", "number"), f("note", "text")];
        let v = json!({ "amount": 1500.0, "note": "Urgent" });
        assert!(compare(&fs, &v, "amount", "gt", "1000"));
        assert!(!compare(&fs, &v, "amount", "lt", "1000"));
        assert!(compare(&fs, &v, "amount", "eq", "1500"));
        assert!(compare(&fs, &v, "note", "eq", "urgent"));
        assert!(compare(&fs, &v, "missing", "empty", ""));
        assert!(compare(&fs, &v, "note", "filled", ""));
    }
}
