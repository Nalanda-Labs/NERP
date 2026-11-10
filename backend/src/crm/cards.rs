//! Record card layouts. A record type's "card fields" (everything except the identity
//! fields at the top of a record: name, amount, pipeline and stage, client, responsible) are
//! grouped into named sections, in order, on the record page and in its form. Fields can be
//! hidden; fields nobody placed yet (e.g. a custom field added later) go to the last section.
//!
//! CRM admins set the layout for everyone; each employee may keep a personal one instead.

use axum::{
    extract::{Path, Query, State},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashSet;

use super::{
    access::{is_crm_admin, require_any},
    entity_type,
    fields::{custom_fields, system_fields_for, FieldDef},
    EntityType,
};
use crate::{
    auth::AuthUser,
    error::{AppError, AppResult},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new().route("/crm/{type}/card-layout", get(get_layout).put(put_layout).delete(reset_layout))
}

#[derive(Serialize, Clone, Debug)]
pub struct CardField {
    pub code: String,
    pub label: String,
    /// Required custom fields cannot be hidden.
    pub required: bool,
}

/// Built-in fields that live on the card (in display order for the default layout).
const SYSTEM: &[&str] = &["close_date", "probability", "company_kind", "industry", "employees", "annual_revenue", "birthdate", "source", "source_details"];

/// The fields a type's card can show.
pub fn catalogue(t: &EntityType, custom: &[FieldDef]) -> Vec<CardField> {
    let sys = system_fields_for(t);
    let mut out: Vec<CardField> = SYSTEM
        .iter()
        .filter_map(|c| sys.iter().find(|f| f.code == *c))
        .map(|f| CardField { code: f.code.into(), label: f.label.into(), required: false })
        .collect();
    out.push(CardField { code: "utm".into(), label: "Campaign (UTM)".into(), required: false });
    for f in custom {
        out.push(CardField { code: format!("uf.{}", f.code), label: f.label.clone(), required: f.is_required });
    }
    if sys.iter().any(|f| f.code == "phone") {
        out.push(CardField { code: "contact_details".into(), label: "Phone, email, website".into(), required: false });
    }
    if sys.iter().any(|f| f.code == "address") {
        out.push(CardField { code: "address".into(), label: "Address".into(), required: false });
    }
    out.push(CardField { code: "comments".into(), label: "Comment".into(), required: false });
    out
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Section {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub fields: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Layout {
    pub sections: Vec<Section>,
    #[serde(default)]
    pub hidden: Vec<String>,
    /// On the record page, leave out fields without a value.
    #[serde(default)]
    pub hide_empty: bool,
}

pub fn default_layout(fields: &[CardField]) -> Layout {
    let codes = |pred: &dyn Fn(&str) -> bool| fields.iter().map(|f| f.code.clone()).filter(|c| pred(c)).collect::<Vec<_>>();
    let mut sections = vec![Section { id: "about".into(), title: "About".into(), fields: codes(&|c| !["contact_details", "address", "comments"].contains(&c)) }];
    let contact = codes(&|c| c == "contact_details" || c == "address");
    if !contact.is_empty() {
        sections.push(Section { id: "contact".into(), title: "Contact details".into(), fields: contact });
    }
    sections.push(Section { id: "more".into(), title: "Comment".into(), fields: vec!["comments".into()] });
    Layout { sections, hidden: vec![], hide_empty: false }
}

/// Cleans a layout against the type's fields. `strict` (when saving) rejects mistakes; when
/// reading a stored layout, they are repaired instead (e.g. a field that was deleted).
pub fn normalize(input: &Layout, fields: &[CardField], strict: bool) -> AppResult<Layout> {
    let known: HashSet<&str> = fields.iter().map(|f| f.code.as_str()).collect();
    let bad = |m: &str| -> AppResult<()> { if strict { Err(AppError::bad(m.to_string())) } else { Ok(()) } };
    if input.sections.len() > 20 {
        bad("at most 20 sections")?;
    }
    let mut seen: HashSet<String> = HashSet::new();
    let mut sections = Vec::new();
    let mut ids: HashSet<String> = HashSet::new();
    for (i, s) in input.sections.iter().take(20).enumerate() {
        let title: String = s.title.trim().chars().take(60).collect();
        if title.is_empty() {
            bad("every section needs a title")?;
        }
        let mut id: String = s.id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').take(40).collect();
        if id.is_empty() || ids.contains(&id) {
            id = format!("s{i}");
        }
        ids.insert(id.clone());
        let mut list = Vec::new();
        for f in &s.fields {
            if !known.contains(f.as_str()) {
                bad(&format!("unknown field {f}"))?;
                continue;
            }
            if !seen.insert(f.clone()) {
                bad(&format!("{f} is placed twice"))?;
                continue;
            }
            list.push(f.clone());
        }
        sections.push(Section { id, title: if title.is_empty() { "Section".into() } else { title }, fields: list });
    }
    let mut hidden = Vec::new();
    for f in &input.hidden {
        let Some(def) = fields.iter().find(|x| &x.code == f) else { continue };
        if def.required {
            bad(&format!("{} is required, so it cannot be hidden", def.label))?;
            continue;
        }
        if seen.insert(f.clone()) {
            hidden.push(f.clone());
        }
    }
    if sections.is_empty() {
        bad("keep at least one section")?;
        sections.push(Section { id: "about".into(), title: "About".into(), fields: vec![] });
    }
    // Fields nobody placed (new custom fields, or hidden required ones) go to the end.
    for f in fields {
        if !seen.contains(&f.code) {
            sections.last_mut().unwrap().fields.push(f.code.clone());
        }
    }
    Ok(Layout { sections, hidden, hide_empty: input.hide_empty })
}

async fn stored(state: &AppState, t: i16, user: Option<uuid::Uuid>) -> AppResult<Option<Layout>> {
    let v: Option<Value> = sqlx::query_scalar("SELECT layout FROM crm_card_layouts WHERE entity_type = $1 AND user_id IS NOT DISTINCT FROM $2")
        .bind(t)
        .bind(user)
        .fetch_optional(&state.db)
        .await?;
    Ok(v.and_then(|v| serde_json::from_value(v).ok()))
}

async fn payload(state: &AppState, auth: &AuthUser, t: &EntityType) -> AppResult<Value> {
    let fields = catalogue(t, &custom_fields(state, t.id).await?);
    let (layout, scope) = match stored(state, t.id, Some(auth.id)).await? {
        Some(l) => (l, "mine"),
        None => match stored(state, t.id, None).await? {
            Some(l) => (l, "common"),
            None => (default_layout(&fields), "default"),
        },
    };
    let has_common = scope == "common" || (scope == "mine" && stored(state, t.id, None).await?.is_some());
    Ok(json!({
        "layout": normalize(&layout, &fields, false)?,
        "scope": scope,
        "has_common": has_common,
        "fields": fields,
        "can_edit_common": is_crm_admin(auth),
    }))
}

async fn get_layout(State(state): State<AppState>, auth: AuthUser, Path(code): Path<String>) -> AppResult<Json<Value>> {
    let t = entity_type(&state, &code).await?;
    require_any(&state, &auth, t.id, "read").await?;
    Ok(Json(payload(&state, &auth, &t).await?))
}

#[derive(Deserialize)]
struct PutBody {
    layout: Layout,
    /// "mine" (default) or "common".
    scope: Option<String>,
}

async fn put_layout(State(state): State<AppState>, auth: AuthUser, Path(code): Path<String>, Json(b): Json<PutBody>) -> AppResult<Json<Value>> {
    let t = entity_type(&state, &code).await?;
    require_any(&state, &auth, t.id, "read").await?;
    let common = match b.scope.as_deref().unwrap_or("mine") {
        "mine" => false,
        "common" if is_crm_admin(&auth) => true,
        "common" => return Err(AppError::Forbidden),
        _ => return Err(AppError::bad("scope must be mine or common")),
    };
    let fields = catalogue(&t, &custom_fields(&state, t.id).await?);
    let layout = serde_json::to_value(normalize(&b.layout, &fields, true)?).unwrap();
    let user = if common { None } else { Some(auth.id) };
    let mut tx = state.db.begin().await?;
    sqlx::query("DELETE FROM crm_card_layouts WHERE entity_type = $1 AND user_id IS NOT DISTINCT FROM $2").bind(t.id).bind(user).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO crm_card_layouts (entity_type, user_id, layout, updated_by) VALUES ($1, $2, $3, $4)")
        .bind(t.id)
        .bind(user)
        .bind(layout)
        .bind(auth.id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    if common {
        crate::audit::record(&state.db, Some(auth.id), "crm.card_layout", "crm_type", Some(t.code.clone()), json!({}), None).await;
    }
    Ok(Json(payload(&state, &auth, &t).await?))
}

#[derive(Deserialize)]
struct ResetQuery {
    scope: Option<String>,
}

/// Drops a personal layout (back to the shared one) or the shared one (back to the default).
async fn reset_layout(State(state): State<AppState>, auth: AuthUser, Path(code): Path<String>, Query(q): Query<ResetQuery>) -> AppResult<Json<Value>> {
    let t = entity_type(&state, &code).await?;
    require_any(&state, &auth, t.id, "read").await?;
    let user = match q.scope.as_deref().unwrap_or("mine") {
        "mine" => Some(auth.id),
        "common" if is_crm_admin(&auth) => None,
        "common" => return Err(AppError::Forbidden),
        _ => return Err(AppError::bad("scope must be mine or common")),
    };
    sqlx::query("DELETE FROM crm_card_layouts WHERE entity_type = $1 AND user_id IS NOT DISTINCT FROM $2").bind(t.id).bind(user).execute(&state.db).await?;
    Ok(Json(payload(&state, &auth, &t).await?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(code: &str, required: bool) -> CardField {
        CardField { code: code.into(), label: code.into(), required }
    }
    fn fields() -> Vec<CardField> {
        vec![f("close_date", false), f("source", false), f("uf.tier", true), f("uf.region", false), f("comments", false)]
    }
    fn sec(title: &str, fields: &[&str]) -> Section {
        Section { id: String::new(), title: title.into(), fields: fields.iter().map(|s| s.to_string()).collect() }
    }

    #[test]
    fn default_puts_comments_last() {
        let l = default_layout(&fields());
        assert_eq!(l.sections[0].fields, vec!["close_date", "source", "uf.tier", "uf.region"]);
        assert_eq!(l.sections.last().unwrap().fields, vec!["comments"]);
    }

    #[test]
    fn unplaced_fields_go_to_the_last_section() {
        let l = Layout { sections: vec![sec("Money", &["source"]), sec("Rest", &["comments"])], hidden: vec!["close_date".into()], hide_empty: true };
        let n = normalize(&l, &fields(), true).unwrap();
        assert_eq!(n.sections[1].fields, vec!["comments", "uf.tier", "uf.region"]);
        assert_eq!(n.hidden, vec!["close_date"]);
        assert!(n.hide_empty);
        assert_eq!(n.sections[0].id, "s0");
    }

    #[test]
    fn strict_rejects_mistakes_lenient_repairs() {
        let twice = Layout { sections: vec![sec("A", &["source", "source"])], hidden: vec![], hide_empty: false };
        assert!(normalize(&twice, &fields(), true).is_err());
        assert_eq!(normalize(&twice, &fields(), false).unwrap().sections[0].fields.iter().filter(|c| *c == "source").count(), 1);
        let required_hidden = Layout { sections: vec![sec("A", &[])], hidden: vec!["uf.tier".into()], hide_empty: false };
        assert!(normalize(&required_hidden, &fields(), true).is_err());
        assert!(normalize(&required_hidden, &fields(), false).unwrap().sections[0].fields.contains(&"uf.tier".to_string()));
        let unknown = Layout { sections: vec![sec("A", &["uf.gone"])], hidden: vec![], hide_empty: false };
        assert!(normalize(&unknown, &fields(), true).is_err());
        let untitled = Layout { sections: vec![sec("  ", &[])], hidden: vec![], hide_empty: false };
        assert!(normalize(&untitled, &fields(), true).is_err());
        assert!(normalize(&Layout { sections: vec![], hidden: vec![], hide_empty: false }, &fields(), true).is_err());
    }
}
