//! An MCP server (Model Context Protocol, Streamable HTTP transport): AI assistants and
//! agents outside NERP — any MCP client — work with the CRM, tasks and the employee list
//! through tools.
//!
//! - `POST /api/mcp` takes JSON-RPC 2.0 messages (`initialize`, `ping`, `tools/list`,
//!   `tools/call`; notifications are acknowledged with 202). There is no server-sent stream
//!   (`GET` answers 405), and no session is kept.
//! - Clients sign in with a personal API key (`Authorization: Bearer nerp_…`). A key sees the
//!   tools its scopes allow, and every tool is an ordinary API request made with the same
//!   key, so the key's scopes, its owner's access rights and the rate limit all apply as they
//!   do for any API call.
//! - Requests from a browser page of another site (an `Origin` that isn't NERP's) are refused.

use axum::{
    body::Bytes,
    extract::State,
    http::{header, HeaderMap, HeaderValue, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde_json::{json, Map, Value};
use std::sync::OnceLock;
use std::time::Duration;

use crate::{api::keys, state::AppState};

pub fn router() -> Router<AppState> {
    Router::new().route("/mcp", post(handle).get(no_stream).delete(no_stream))
}

const VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26"];
const MAX_RESULT: usize = 100_000;

struct Tool {
    name: &'static str,
    title: &'static str,
    description: &'static str,
    /// The key scope it needs.
    scope: &'static str,
    method: Method,
    /// With `{arg}` parts filled from the arguments.
    path: &'static str,
    /// Arguments sent in the query string.
    query: &'static [&'static str],
    /// Arguments sent as the JSON body (`fields` is merged in whole).
    body: &'static [&'static str],
    schema: fn() -> Value,
}

fn s(desc: &str) -> Value {
    json!({ "type": "string", "description": desc })
}
fn id(desc: &str) -> Value {
    json!({ "type": "string", "format": "uuid", "description": desc })
}
fn crm_type() -> Value {
    json!({ "type": "string", "description": "Record type code: lead, deal, contact, company, or a smart process's code (see crm_record_types)" })
}
fn obj(props: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": props, "required": required, "additionalProperties": false })
}

fn tools() -> &'static [Tool] {
    static T: OnceLock<Vec<Tool>> = OnceLock::new();
    T.get_or_init(|| {
        vec![
            Tool { name: "crm_record_types", title: "CRM record types", description: "The CRM's record types (leads, deals, contacts, companies, smart processes) with their pipelines, stages and custom fields.", scope: "crm:read", method: Method::GET, path: "/crm/config", query: &[], body: &[], schema: || obj(json!({}), &[]) },
            Tool { name: "crm_search", title: "Find CRM records", description: "Searches records of one type by name, phone, email or #number; newest first.", scope: "crm:read", method: Method::GET, path: "/crm/{type}/items", query: &["q", "limit", "offset", "stage_id"], body: &[], schema: || obj(json!({ "type": crm_type(), "q": s("Text to look for"), "limit": { "type": "integer", "minimum": 1, "maximum": 200 }, "offset": { "type": "integer", "minimum": 0 }, "stage_id": id("Only records in this stage") }), &["type"]) },
            Tool { name: "crm_get", title: "Open a CRM record", description: "A record with its fields, contacts, company and custom fields.", scope: "crm:read", method: Method::GET, path: "/crm/{type}/items/{id}", query: &[], body: &[], schema: || obj(json!({ "type": crm_type(), "id": id("The record") }), &["type", "id"]) },
            Tool { name: "crm_timeline", title: "A CRM record's history", description: "A record's timeline: comments, stage changes, emails, activities and edits, newest first.", scope: "crm:read", method: Method::GET, path: "/crm/items/{id}/timeline", query: &[], body: &[], schema: || obj(json!({ "id": id("The record") }), &["id"]) },
            Tool { name: "crm_create", title: "Make a CRM record", description: "Makes a record. `fields` as in the API: e.g. title, first_name, last_name, company_name, opportunity, currency, stage_id, assigned_to, comments, multifields [{kind: phone|email, value}], uf {custom fields}.", scope: "crm", method: Method::POST, path: "/crm/{type}/items", query: &[], body: &["fields"], schema: || obj(json!({ "type": crm_type(), "fields": { "type": "object", "description": "The record's fields" } }), &["type", "fields"]) },
            Tool { name: "crm_update", title: "Change a CRM record", description: "Changes the given fields of a record (e.g. stage_id to move it, assigned_to, opportunity).", scope: "crm", method: Method::PATCH, path: "/crm/{type}/items/{id}", query: &[], body: &["fields"], schema: || obj(json!({ "type": crm_type(), "id": id("The record"), "fields": { "type": "object", "description": "Fields to change" } }), &["type", "id", "fields"]) },
            Tool { name: "crm_comment", title: "Comment on a CRM record", description: "Adds a comment to a record's timeline (its followers are told).", scope: "crm", method: Method::POST, path: "/crm/items/{id}/timeline", query: &[], body: &["body"], schema: || obj(json!({ "id": id("The record"), "body": s("The comment") }), &["id", "body"]) },
            Tool { name: "crm_add_activity", title: "Plan a CRM activity", description: "Plans a to-do, call or meeting on a record, with a deadline (ISO 8601 time).", scope: "crm", method: Method::POST, path: "/crm/activities", query: &[], body: &["item_id", "kind", "subject", "description", "deadline", "responsible_id"], schema: || obj(json!({ "item_id": id("The record"), "kind": { "type": "string", "enum": ["todo", "call", "meeting"] }, "subject": s("What it is"), "description": s("Details"), "deadline": { "type": "string", "format": "date-time" }, "responsible_id": id("Who does it (default: you)") }), &["item_id", "kind", "subject"]) },
            Tool { name: "tasks_search", title: "Find tasks", description: "Tasks you can see: yours by default, or all you may see (view=all); by text, status (new, in_progress, review, done, deferred; comma-separated), project or responsible person.", scope: "tasks:read", method: Method::GET, path: "/tasks", query: &["q", "status", "view", "project_id", "responsible_id", "overdue", "limit", "offset"], body: &[], schema: || obj(json!({ "q": s("Text to look for"), "status": s("Statuses, comma-separated"), "view": { "type": "string", "enum": ["my", "created", "assisting", "watching", "all"] }, "project_id": id("Only this project's"), "responsible_id": id("Only this person's"), "overdue": { "type": "string", "enum": ["true"] }, "limit": { "type": "integer", "minimum": 1, "maximum": 200 }, "offset": { "type": "integer", "minimum": 0 } }), &[]) },
            Tool { name: "task_get", title: "Open a task", description: "A task with its description, people, deadline, checklist and status.", scope: "tasks:read", method: Method::GET, path: "/tasks/{id}", query: &[], body: &[], schema: || obj(json!({ "id": id("The task") }), &["id"]) },
            Tool { name: "projects_list", title: "Projects and workgroups", description: "Projects and workgroups you can see.", scope: "tasks:read", method: Method::GET, path: "/projects", query: &["q", "limit"], body: &[], schema: || obj(json!({ "q": s("Text to look for"), "limit": { "type": "integer", "minimum": 1, "maximum": 200 } }), &[]) },
            Tool { name: "task_create", title: "Make a task", description: "Sets a task for someone (default: you), optionally in a project, with a deadline (ISO 8601 time) and priority (low, normal, high).", scope: "tasks", method: Method::POST, path: "/tasks", query: &[], body: &["title", "description", "responsible_id", "deadline", "priority", "project_id"], schema: || obj(json!({ "title": s("What to do"), "description": s("Details"), "responsible_id": id("Who does it"), "deadline": { "type": "string", "format": "date-time" }, "priority": { "type": "string", "enum": ["low", "normal", "high"] }, "project_id": id("Project or workgroup") }), &["title"]) },
            Tool { name: "task_update", title: "Change a task", description: "Changes the given fields of a task (e.g. title, description, deadline, responsible_id, priority); its status changes with task_set_status.", scope: "tasks", method: Method::PATCH, path: "/tasks/{id}", query: &[], body: &["fields"], schema: || obj(json!({ "id": id("The task"), "fields": { "type": "object", "description": "Fields to change" } }), &["id", "fields"]) },
            Tool { name: "task_set_status", title: "Move a task on", description: "Changes a task's status: new, in_progress, review (done, for the creator to check), done, or deferred. Those it concerns are told.", scope: "tasks", method: Method::POST, path: "/tasks/{id}/status", query: &[], body: &["status"], schema: || obj(json!({ "id": id("The task"), "status": { "type": "string", "enum": ["new", "in_progress", "review", "done", "deferred"] } }), &["id", "status"]) },
            Tool { name: "task_comment", title: "Comment on a task", description: "Adds a comment to a task (its people are told).", scope: "tasks", method: Method::POST, path: "/tasks/{id}/comments", query: &[], body: &["body"], schema: || obj(json!({ "id": id("The task"), "body": s("The comment") }), &["id", "body"]) },
            Tool { name: "employees_search", title: "Find employees", description: "Employees by name or email, with job title and departments.", scope: "users:read", method: Method::GET, path: "/users", query: &["q", "limit"], body: &[], schema: || obj(json!({ "q": s("Name or email"), "limit": { "type": "integer", "minimum": 1, "maximum": 200 } }), &[]) },
            Tool { name: "departments_list", title: "Company structure", description: "The company's departments, with their heads.", scope: "users:read", method: Method::GET, path: "/departments", query: &[], body: &[], schema: || obj(json!({}), &[]) },
        ]
    })
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn json_response(status: StatusCode, v: Value) -> Response {
    let mut r = (status, Json(v)).into_response();
    r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    r
}

/// No server-to-client stream and no sessions; still, only key holders learn that.
async fn no_stream(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(r) = authorize(&state, &headers).await {
        return r;
    }
    let mut r = StatusCode::METHOD_NOT_ALLOWED.into_response();
    r.headers_mut().insert(header::ALLOW, HeaderValue::from_static("POST"));
    r
}

fn unauthorized(msg: &str) -> Response {
    let mut r = json_response(StatusCode::UNAUTHORIZED, json!({ "error": msg }));
    r.headers_mut().insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer realm=\"NERP\""));
    r
}

/// The request's key and its scopes: from NERP's own pages or non-browser clients only, and a
/// key with at least one tool here (a key for, say, files alone has nothing to do).
async fn authorize(state: &AppState, headers: &HeaderMap) -> Result<(String, Vec<String>), Response> {
    if let Some(o) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok())
        && o.trim_end_matches('/') != state.config.public_url.trim_end_matches('/')
    {
        return Err(json_response(StatusCode::FORBIDDEN, json!({ "error": "requests from other sites aren't allowed" })));
    }
    let Some(auth) = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).map(str::to_string) else {
        return Err(unauthorized("sign in with an API key: Authorization: Bearer nerp_…"));
    };
    let Some(token) = auth.strip_prefix("Bearer ").map(str::trim).filter(|t| t.starts_with(keys::PREFIX)) else {
        return Err(unauthorized("use a personal API key (Profile → API keys)"));
    };
    let scopes = keys::key_scopes(state, token)
        .await
        .map_err(|_| unauthorized("this API key isn't valid (revoked, expired, or its owner can't use the API)"))?;
    if !tools().iter().any(|t| keys::allows(&scopes, t.scope)) {
        return Err(json_response(StatusCode::FORBIDDEN, json!({ "error": "this key's scopes give it no tools here (CRM, tasks or employees)" })));
    }
    Ok((auth, scopes))
}

async fn handle(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    let (auth, scopes) = match authorize(&state, &headers).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    let Ok(msg) = serde_json::from_slice::<Value>(&body) else {
        return json_response(StatusCode::BAD_REQUEST, rpc_error(Value::Null, -32700, "parse error"));
    };
    match msg {
        Value::Array(list) => {
            let mut out = Vec::new();
            for m in list.into_iter().take(50) {
                if let Some(r) = one(&state, &auth, &scopes, m).await {
                    out.push(r);
                }
            }
            if out.is_empty() { StatusCode::ACCEPTED.into_response() } else { json_response(StatusCode::OK, Value::Array(out)) }
        }
        m => match one(&state, &auth, &scopes, m).await {
            Some(r) => json_response(StatusCode::OK, r),
            None => StatusCode::ACCEPTED.into_response(),
        },
    }
}

/// Answers one message (None for notifications and responses, which get no answer).
async fn one(state: &AppState, auth: &str, scopes: &[String], m: Value) -> Option<Value> {
    let id = m.get("id").cloned();
    let method = m.get("method").and_then(Value::as_str).unwrap_or("");
    let Some(id) = id.filter(|i| !i.is_null()) else { return None };
    if m.get("jsonrpc").and_then(Value::as_str) != Some("2.0") || method.is_empty() {
        return Some(rpc_error(id, -32600, "invalid request"));
    }
    let params = m.get("params").cloned().unwrap_or(json!({}));
    Some(match method {
        "initialize" => {
            let asked = params["protocolVersion"].as_str().unwrap_or("");
            let version = if VERSIONS.contains(&asked) { asked } else { VERSIONS[0] };
            json!({ "jsonrpc": "2.0", "id": id, "result": {
                "protocolVersion": version,
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": { "name": "nerp", "title": "NERP", "version": env!("CARGO_PKG_VERSION") },
                "instructions": "Tools for the company's CRM (records, comments, activities), tasks and projects, and employee directory, acting as the API key's owner with their access rights. Times are ISO 8601. Use crm_record_types first to learn record types, stages and custom fields.",
            } })
        }
        "ping" => json!({ "jsonrpc": "2.0", "id": id, "result": {} }),
        "tools/list" => {
            let list: Vec<Value> = tools()
                .iter()
                .filter(|t| keys::allows(scopes, t.scope))
                .map(|t| json!({
                    "name": t.name, "title": t.title, "description": t.description, "inputSchema": (t.schema)(),
                    "annotations": { "readOnlyHint": t.method == Method::GET, "destructiveHint": false, "openWorldHint": false },
                }))
                .collect();
            json!({ "jsonrpc": "2.0", "id": id, "result": { "tools": list } })
        }
        "tools/call" => {
            let name = params["name"].as_str().unwrap_or("");
            let Some(t) = tools().iter().find(|t| t.name == name && keys::allows(scopes, t.scope)) else {
                return Some(rpc_error(id, -32602, &format!("unknown tool: {name}")));
            };
            let args = params["arguments"].as_object().cloned().unwrap_or_default();
            match call(state, auth, t, &args).await {
                Ok(v) => json!({ "jsonrpc": "2.0", "id": id, "result": v }),
                Err(e) => rpc_error(id, -32602, &e),
            }
        }
        _ => rpc_error(id, -32601, &format!("method not found: {method}")),
    })
}

fn http() -> &'static reqwest::Client {
    static C: OnceLock<reqwest::Client> = OnceLock::new();
    C.get_or_init(|| {
        // Built like the app's other clients (its TLS set-up), though it only calls itself.
        let tls = crate::crm::email::imap::client_config(false).expect("TLS set-up");
        reqwest::Client::builder()
            .tls_backend_preconfigured(tls)
            .timeout(Duration::from_secs(60))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent("NERP-MCP/1")
            .build()
            .expect("HTTP client")
    })
}

/// Where this server answers on the machine itself.
fn loopback(state: &AppState) -> String {
    let a = state.config.bind_addr;
    let host = if a.ip().is_unspecified() { if a.is_ipv6() { "[::1]".to_string() } else { "127.0.0.1".to_string() } } else if a.is_ipv6() { format!("[{}]", a.ip()) } else { a.ip().to_string() };
    format!("http://{host}:{}", a.port())
}

fn arg_text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// Runs a tool as an API request with the caller's key. Errors in the arguments are protocol
/// errors; refusals and failures from the API are tool results with `isError`.
async fn call(state: &AppState, auth: &str, t: &Tool, args: &Map<String, Value>) -> Result<Value, String> {
    let schema = (t.schema)();
    for r in schema["required"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        if args.get(r).is_none_or(Value::is_null) {
            return Err(format!("missing argument: {r}"));
        }
    }
    let known: Vec<&String> = schema["properties"].as_object().map(|p| p.keys().collect()).unwrap_or_default();
    if let Some(k) = args.keys().find(|k| !known.contains(k)) {
        return Err(format!("unknown argument: {k}"));
    }
    // The path: ids must be ids, type codes plain words.
    let mut path = t.path.to_string();
    for (k, v) in args {
        let key = format!("{{{k}}}");
        if path.contains(&key) {
            let text = v.as_str().ok_or_else(|| format!("{k} must be text"))?;
            let ok = if k == "type" { !text.is_empty() && text.len() <= 50 && text.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') } else { uuid::Uuid::parse_str(text).is_ok() };
            if !ok {
                return Err(format!("{k} isn't valid"));
            }
            path = path.replace(&key, text);
        }
    }
    let mut url = reqwest::Url::parse(&format!("{}/api{path}", loopback(state))).map_err(|_| "bad path".to_string())?;
    {
        let mut q = url.query_pairs_mut();
        for k in t.query {
            if let Some(v) = args.get(*k).and_then(arg_text) {
                q.append_pair(k, &v);
            }
        }
    }
    if url.query() == Some("") {
        url.set_query(None);
    }
    let body = if t.body.is_empty() {
        None
    } else {
        let mut b = Map::new();
        for k in t.body {
            match (k, args.get(*k)) {
                (&"fields", Some(Value::Object(f))) => b.extend(f.clone()),
                (&"fields", Some(_)) => return Err("fields must be an object".into()),
                (_, Some(v)) if !v.is_null() => {
                    b.insert(k.to_string(), v.clone());
                }
                _ => {}
            }
        }
        Some(Value::Object(b))
    };
    let mut req = http().request(t.method.clone(), url).header(header::AUTHORIZATION, auth).header("X-NERP-Via", "mcp");
    if let Some(b) = &body {
        req = req.json(b);
    }
    let res = match req.send().await {
        Ok(r) => r,
        Err(e) => return Ok(json!({ "content": [{ "type": "text", "text": format!("NERP didn't answer: {e}") }], "isError": true })),
    };
    let status = res.status();
    let text = res.text().await.unwrap_or_default();
    let parsed: Option<Value> = serde_json::from_str(&text).ok();
    if !status.is_success() {
        let msg = parsed.as_ref().and_then(|v| v["error"]["message"].as_str().map(String::from)).unwrap_or_else(|| text.chars().take(500).collect());
        return Ok(json!({ "content": [{ "type": "text", "text": format!("{} ({})", msg, status.as_u16()) }], "isError": true }));
    }
    let mut out_text = text;
    if out_text.len() > MAX_RESULT {
        out_text = format!("{}… (cut short: use limit/offset or a narrower search)", out_text.chars().take(MAX_RESULT).collect::<String>());
    }
    let mut r = json!({ "content": [{ "type": "text", "text": out_text }], "isError": false });
    if let Some(v @ Value::Object(_)) = parsed {
        r["structuredContent"] = v;
    }
    Ok(r)
}

#[cfg(test)]
mod tests {
    #[test]
    fn tool_names_are_unique_and_schemas_objects() {
        let ts = super::tools();
        let mut names: Vec<&str> = ts.iter().map(|t| t.name).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), ts.len());
        for t in ts {
            assert_eq!((t.schema)()["type"], "object", "{}", t.name);
            assert!(t.name.chars().all(|c| c.is_ascii_lowercase() || c == '_'));
        }
    }
}
