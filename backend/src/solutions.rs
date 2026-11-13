//! Ready-made solutions: a list with its fields and its approval process, made in one go —
//! expense claims, business trips, invoices to pay, outgoing documents, hiring requests and
//! purchase requests. Each is an ordinary list afterwards (fields and steps can be changed).
//!
//! Steps go to the author's department head, and to the people named for finance, HR or the
//! office when given (else the head again).

use axum::{
    extract::{Path, State},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{
    auth::{permissions as perm, AuthUser},
    error::{AppError, AppResult},
    lists::{save_fields, Field},
    state::AppState,
    workflows::clean_steps,
};

pub fn router() -> Router<AppState> {
    Router::new().route("/lists/solutions", get(catalogue)).route("/lists/solutions/{key}", post(install))
}

struct Solution {
    key: &'static str,
    name: &'static str,
    description: &'static str,
    /// Who else it asks for: finance, hr, office.
    roles: &'static [&'static str],
}

const ALL: &[Solution] = &[
    Solution { key: "expenses", name: "Expense claims", description: "Employees claim what they paid; their head approves, finance approves larger sums and pays.", roles: &["finance"] },
    Solution { key: "trips", name: "Business trips", description: "Ask for a trip; the head approves; the office books travel.", roles: &["office"] },
    Solution { key: "invoices", name: "Invoices to pay", description: "Supplier invoices go to the head for approval, then to finance to pay by the due date.", roles: &["finance"] },
    Solution { key: "outgoing", name: "Outgoing documents", description: "Letters and contracts are approved before they're sent, with a record of what went where.", roles: &[] },
    Solution { key: "hiring", name: "Hiring requests", description: "A head asks to hire; HR approves and starts recruiting.", roles: &["hr"] },
    Solution { key: "purchases", name: "Purchase requests", description: "Ask to buy something; the head approves, finance approves larger sums, the office orders it.", roles: &["finance", "office"] },
];

fn f(code: &str, label: &str, kind: &str, options: &[&str], required: bool) -> Field {
    Field { id: None, code: code.into(), label: label.into(), kind: kind.into(), options: options.iter().map(|o| o.to_string()).collect(), required }
}

/// The fields and the process of a solution; `who(role)` names people for finance, HR or the office.
fn build(key: &str, who: &dyn Fn(&str) -> Value) -> Option<(Vec<Field>, Value)> {
    let head = json!({ "head": true });
    let author = json!({ "author": true });
    let done = |status: &str, text: &str| json!([{ "kind": "set_field", "field": "status", "value": status }, { "kind": "notify", "who": author, "text": text }]);
    Some(match key {
        "expenses" => (
            vec![
                f("amount", "Amount", "money", &[], true),
                f("spent_on", "Date", "date", &[], true),
                f("category", "Category", "select", &["Travel", "Meals", "Office", "Training", "Other"], true),
                f("receipt", "Receipt", "file", &[], false),
                f("details", "Details", "textarea", &[], false),
                f("status", "Status", "select", &["Waiting", "Approved", "Rejected", "Paid"], false),
            ],
            json!([{ "kind": "set_field", "field": "status", "value": "Waiting" },
                { "kind": "approval", "title": "Approve the expense claim “{{title}}” ({{amount}})", "who": head, "due_hours": 72,
                  "approved": [{ "kind": "condition", "field": "amount", "op": "gt", "value": "500",
                      "then": [{ "kind": "approval", "title": "Finance: approve “{{title}}” ({{amount}})", "who": who("finance"),
                          "approved": [{ "kind": "set_field", "field": "status", "value": "Approved" }, { "kind": "task", "who": who("finance"), "title": "Pay the expense claim “{{title}}” to {{author}}", "deadline_hours": 120 }, { "kind": "notify", "who": author, "text": "Your expense claim “{{title}}” was approved" }],
                          "rejected": done("Rejected", "Your expense claim “{{title}}” was not approved") }],
                      "else": [{ "kind": "set_field", "field": "status", "value": "Approved" }, { "kind": "task", "who": who("finance"), "title": "Pay the expense claim “{{title}}” to {{author}}", "deadline_hours": 120 }, { "kind": "notify", "who": author, "text": "Your expense claim “{{title}}” was approved" }] }],
                  "rejected": done("Rejected", "Your expense claim “{{title}}” was not approved") }]),
        ),
        "trips" => (
            vec![
                f("destination", "Destination", "text", &[], true),
                f("starts_on", "From", "date", &[], true),
                f("ends_on", "To", "date", &[], true),
                f("purpose", "Purpose", "textarea", &[], true),
                f("budget", "Budget", "money", &[], false),
                f("status", "Status", "select", &["Requested", "Approved", "Rejected"], false),
            ],
            json!([{ "kind": "set_field", "field": "status", "value": "Requested" },
                { "kind": "approval", "title": "Approve the trip to {{destination}} ({{starts_on}} – {{ends_on}})", "who": head, "due_hours": 48,
                  "approved": [{ "kind": "set_field", "field": "status", "value": "Approved" }, { "kind": "task", "who": who("office"), "title": "Book travel to {{destination}} for {{author}}", "description": "{{starts_on}} – {{ends_on}}. {{purpose}}", "deadline_hours": 48 }, { "kind": "notify", "who": author, "text": "Your trip to {{destination}} was approved" }],
                  "rejected": done("Rejected", "Your trip to {{destination}} was not approved") }]),
        ),
        "invoices" => (
            vec![
                f("supplier", "Supplier", "text", &[], true),
                f("invoice_no", "Invoice number", "text", &[], false),
                f("amount", "Amount", "money", &[], true),
                f("due_on", "Due date", "date", &[], true),
                f("invoice", "Invoice", "file", &[], false),
                f("status", "Status", "select", &["To approve", "Approved", "Rejected", "Paid"], false),
            ],
            json!([{ "kind": "set_field", "field": "status", "value": "To approve" },
                { "kind": "approval", "title": "Approve paying {{supplier}} {{amount}} by {{due_on}}", "who": head, "due_hours": 48,
                  "approved": [{ "kind": "set_field", "field": "status", "value": "Approved" }, { "kind": "task", "who": who("finance"), "title": "Pay invoice {{invoice_no}} to {{supplier}} ({{amount}}) by {{due_on}}", "deadline_hours": 72 }],
                  "rejected": done("Rejected", "Paying {{supplier}} was not approved") }]),
        ),
        "outgoing" => (
            vec![
                f("recipient", "Recipient", "text", &[], true),
                f("doc_kind", "Kind", "select", &["Letter", "Contract", "Notice", "Other"], true),
                f("document", "Document", "file", &[], true),
                f("sent_on", "Sent on", "date", &[], false),
                f("status", "Status", "select", &["Draft", "Approved", "Rejected", "Sent"], false),
            ],
            json!([{ "kind": "set_field", "field": "status", "value": "Draft" },
                { "kind": "approval", "title": "Approve sending “{{title}}” to {{recipient}}", "who": head, "due_hours": 48,
                  "approved": [{ "kind": "set_field", "field": "status", "value": "Approved" }, { "kind": "task", "who": author, "title": "Send “{{title}}” to {{recipient}}", "deadline_hours": 48 }],
                  "rejected": done("Rejected", "Sending “{{title}}” was not approved") }]),
        ),
        "hiring" => (
            vec![
                f("position", "Position", "text", &[], true),
                f("department", "Department", "text", &[], false),
                f("headcount", "How many", "number", &[], true),
                f("salary", "Salary range", "text", &[], false),
                f("start_on", "Start date", "date", &[], false),
                f("reason", "Why", "textarea", &[], true),
                f("status", "Status", "select", &["Requested", "Approved", "Rejected", "Filled"], false),
            ],
            json!([{ "kind": "set_field", "field": "status", "value": "Requested" },
                { "kind": "approval", "title": "Approve hiring {{headcount}} × {{position}}", "who": head, "due_hours": 72,
                  "approved": [{ "kind": "approval", "title": "HR: approve hiring {{headcount}} × {{position}}", "who": who("hr"), "due_hours": 72,
                      "approved": [{ "kind": "set_field", "field": "status", "value": "Approved" }, { "kind": "task", "who": who("hr"), "title": "Start recruiting: {{position}} ({{headcount}})", "description": "{{reason}}", "deadline_hours": 120 }, { "kind": "notify", "who": author, "text": "Hiring {{position}} was approved" }],
                      "rejected": done("Rejected", "Hiring {{position}} was not approved") }],
                  "rejected": done("Rejected", "Hiring {{position}} was not approved") }]),
        ),
        "purchases" => (
            vec![
                f("what", "What", "text", &[], true),
                f("amount", "Cost", "money", &[], true),
                f("supplier", "Supplier", "text", &[], false),
                f("link", "Link", "text", &[], false),
                f("needed_by", "Needed by", "date", &[], false),
                f("status", "Status", "select", &["Requested", "Approved", "Rejected", "Ordered"], false),
            ],
            json!([{ "kind": "set_field", "field": "status", "value": "Requested" },
                { "kind": "approval", "title": "Approve buying {{what}} ({{amount}})", "who": head, "due_hours": 48,
                  "approved": [{ "kind": "condition", "field": "amount", "op": "gt", "value": "1000",
                      "then": [{ "kind": "approval", "title": "Finance: approve buying {{what}} ({{amount}})", "who": who("finance"),
                          "approved": [{ "kind": "set_field", "field": "status", "value": "Approved" }, { "kind": "task", "who": who("office"), "title": "Order {{what}} from {{supplier}}", "description": "{{link}}", "deadline_hours": 72 }],
                          "rejected": done("Rejected", "Buying {{what}} was not approved") }],
                      "else": [{ "kind": "set_field", "field": "status", "value": "Approved" }, { "kind": "task", "who": who("office"), "title": "Order {{what}} from {{supplier}}", "description": "{{link}}", "deadline_hours": 72 }] }],
                  "rejected": done("Rejected", "Buying {{what}} was not approved") }]),
        ),
        _ => return None,
    })
}

async fn catalogue(auth: AuthUser) -> AppResult<Json<Value>> {
    auth.require(perm::LISTS)?;
    Ok(Json(json!({ "items": ALL.iter().map(|s| json!({ "key": s.key, "name": s.name, "description": s.description, "roles": s.roles })).collect::<Vec<_>>() })))
}

#[derive(Deserialize, Default)]
struct InstallInput {
    name: Option<String>,
    #[serde(default)]
    finance: Vec<Uuid>,
    #[serde(default)]
    hr: Vec<Uuid>,
    #[serde(default)]
    office: Vec<Uuid>,
}

async fn install(State(state): State<AppState>, auth: AuthUser, Path(key): Path<String>, body: Option<Json<InstallInput>>) -> AppResult<Json<Value>> {
    auth.require(perm::LISTS)?;
    let b = body.map(|b| b.0).unwrap_or_default();
    let sol = ALL.iter().find(|s| s.key == key).ok_or(AppError::NotFound("solution"))?;
    let people = |ids: &[Uuid]| if ids.is_empty() { json!({ "head": true }) } else { json!({ "users": ids }) };
    let (finance, hr, office) = (people(&b.finance), people(&b.hr), people(&b.office));
    let who = |role: &str| match role {
        "finance" => finance.clone(),
        "hr" => hr.clone(),
        _ => office.clone(),
    };
    let (fields, steps) = build(&key, &who).ok_or(AppError::NotFound("solution"))?;
    let name = crate::routes::required("name", b.name.as_deref().unwrap_or(sol.name), 120)?;
    let mut tx = state.db.begin().await?;
    let id: Uuid = sqlx::query_scalar!("INSERT INTO lists (name, description, to_all, created_by) VALUES ($1, $2, TRUE, $3) RETURNING id", &name, sol.description, auth.id)
        .fetch_one(&mut *tx)
        .await?;
    save_fields(&mut tx, id, &fields).await?;
    let saved: Vec<Field> = sqlx::query_as!(Field, r#"SELECT id AS "id?", code, label, kind, options, required FROM list_fields WHERE list_id = $1 ORDER BY sort"#, id).fetch_all(&mut *tx).await?;
    let steps = clean_steps(&steps, &saved, 0, &mut 0)?;
    sqlx::query!("INSERT INTO wf_templates (list_id, name, trigger, steps, is_active, created_by) VALUES ($1, $2, 'create', $3, TRUE, $4)", id, format!("{} approval", sol.name), &steps, auth.id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(json!({ "id": id, "name": name })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_solution_is_a_valid_list_and_process() {
        let who = |_: &str| json!({ "head": true });
        for s in ALL {
            let (fields, steps) = build(s.key, &who).expect(s.key);
            clean_steps(&steps, &fields, 0, &mut 0).unwrap_or_else(|e| panic!("{}: {e:?}", s.key));
        }
    }
}
