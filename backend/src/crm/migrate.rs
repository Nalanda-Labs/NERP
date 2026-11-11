//! Moving over from another CRM: the export of HubSpot, Salesforce, Pipedrive or Zoho CRM
//! (or any CSV files) — companies, contacts, leads, deals, activities and notes — with the
//! links between them kept.
//!
//! 1. `analyze`: the uploaded files (CSV, or ZIP archives of CSVs) are read; each file's kind
//!    and each column's field are recognised from the system's own column names, and the
//!    values that need a decision are listed: deal pipelines and stages, lead statuses and
//!    the people records belong to (matched to employees by email or name, or through a
//!    users file in the export).
//! 2. The person checks and adjusts that plan; `start` runs it as a background job, in order
//!    (companies, contacts, leads, deals, then activities and notes), quietly: no robots,
//!    business processes, webhooks or notifications. Every record's id in the old system is
//!    kept (crm_migration_ids), so links resolve, and running the same export again updates
//!    (or skips) instead of duplicating. Records that already exist in NERP are matched by
//!    email/phone (people) or name (companies).
//! 3. `undo` moves everything a migration created to the recycle bin (activities and notes
//!    are removed); records it only updated stay.

use anyhow::{anyhow, Result};
use axum::{
    extract::{Path, State},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap};
use uuid::Uuid;

use super::{access::is_crm_admin, entity_type_by_id, items::{import_item, normalize}, COMPANY, CONTACT, DEAL, LEAD};
use crate::{
    auth::AuthUser,
    error::{AppError, AppResult},
    jobs,
    state::AppState,
};

pub const JOB: &str = "crm.migrate";
const SOURCES: &[&str] = &["hubspot", "salesforce", "pipedrive", "zoho", "other"];
/// Kinds in the order they are brought over (links point backwards).
const KINDS: &[&str] = &["user", "company", "contact", "lead", "deal", "activity", "note"];

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/crm/migrations", get(list).post(start))
        .route("/crm/migrations/analyze", post(analyze))
        .route("/crm/migrations/{id}", get(one))
        .route("/crm/migrations/{id}/undo", post(undo))
}

fn require_admin(auth: &AuthUser) -> AppResult<()> {
    if is_crm_admin(auth) { Ok(()) } else { Err(AppError::Forbidden) }
}

// ---------------------------------------------------------------------------
// Reading the export
// ---------------------------------------------------------------------------

struct Table {
    name: String,
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
}

/// The CSV files in the uploads (ZIP archives are opened; other files are ignored).
async fn tables(state: &AppState, auth: &AuthUser, file_ids: &[Uuid]) -> AppResult<Vec<Table>> {
    let mut out = Vec::new();
    for id in file_ids {
        let f = crate::routes::files::fetch(state, *id).await?;
        if f.owner_id != Some(auth.id) {
            return Err(AppError::bad("upload the files first"));
        }
        let bytes = crate::routes::files::read_content(state, &f).await?;
        if bytes.starts_with(b"PK\x03\x04") {
            let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| AppError::bad(format!("{}: not a readable ZIP ({e})", f.original_name)))?;
            for i in 0..zip.len() {
                let mut entry = zip.by_index(i).map_err(|e| AppError::bad(e.to_string()))?;
                let name = entry.name().map(|n| n.to_string()).unwrap_or_default().rsplit('/').next().unwrap_or("").to_string();
                if !entry.is_file() || !name.to_lowercase().ends_with(".csv") || name.starts_with("._") {
                    continue;
                }
                if entry.size() > 500 * 1024 * 1024 {
                    return Err(AppError::bad(format!("{name} is larger than 500 MB")));
                }
                let mut buf = Vec::new();
                std::io::Read::read_to_end(&mut entry, &mut buf).map_err(|e| AppError::bad(e.to_string()))?;
                out.push(parse_csv(name, &buf)?);
            }
        } else {
            out.push(parse_csv(f.original_name.clone(), &bytes)?);
        }
    }
    if out.is_empty() {
        return Err(AppError::bad("no CSV files found in the upload"));
    }
    Ok(out)
}

fn parse_csv(name: String, bytes: &[u8]) -> AppResult<Table> {
    let text = super::vcard::decode(bytes);
    let first = text.lines().next().unwrap_or("");
    // Exports use commas; some (Excel in many countries) semicolons or tabs.
    let delimiter = [b',', b';', b'\t'].into_iter().max_by_key(|d| first.bytes().filter(|b| b == d).count()).unwrap_or(b',');
    let mut rdr = csv::ReaderBuilder::new().delimiter(delimiter).flexible(true).from_reader(text.as_bytes());
    let headers: Vec<String> = rdr.headers().map_err(|e| AppError::bad(format!("{name}: {e}")))?.iter().map(|h| h.trim().trim_start_matches('\u{feff}').to_string()).collect();
    let mut rows = Vec::new();
    for rec in rdr.records() {
        let rec = rec.map_err(|e| AppError::bad(format!("{name}: {e}")))?;
        if rec.iter().all(|v| v.trim().is_empty()) {
            continue;
        }
        rows.push(rec.iter().map(|v| v.trim().to_string()).collect());
    }
    Ok(Table { name, headers, rows })
}

/// "Deal - Expected close date" → "expectedclosedate" (and the form with the prefix).
fn norm(h: &str) -> (String, String) {
    let full: String = h.to_lowercase().chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    let lower = h.to_lowercase();
    let short = ["organization - ", "person - ", "deal - ", "lead - ", "activity - ", "note - ", "product - "]
        .iter()
        .find_map(|p| lower.strip_prefix(p))
        .map(|s| s.chars().filter(|c| c.is_ascii_alphanumeric()).collect())
        .unwrap_or_else(|| full.clone());
    (full, short)
}

/// Column names each field goes by, in the systems' exports (lowercase, letters and digits).
fn synonyms(kind: &str) -> &'static [(&'static str, &'static [&'static str])] {
    const ADDRESS: [(&str, &[&str]); 5] = [
        ("address.line1", &["street", "streetaddress", "address", "billingstreet", "mailingstreet", "address1"]),
        ("address.city", &["city", "billingcity", "mailingcity", "addresslocality"]),
        ("address.region", &["stateregion", "state", "billingstate", "mailingstate", "region", "province", "stateprovince"]),
        ("address.postal_code", &["postalcode", "zip", "zipcode", "billingpostalcode", "mailingpostalcode", "billingcode", "mailingzip", "postcode"]),
        ("address.country", &["countryregion", "country", "billingcountry", "mailingcountry"]),
    ];
    static COMPANY_F: std::sync::LazyLock<Vec<(&str, &[&str])>> = std::sync::LazyLock::new(|| {
        let mut v: Vec<(&str, &[&str])> = vec![
            ("id", &["recordid", "id", "organizationid", "companyid", "accountid"]),
            ("name", &["companyname", "name", "accountname", "organizationname", "company", "organization"]),
            ("phone", &["phone", "phonenumber", "companyphone", "telephone", "mainphone"]),
            ("email", &["email", "companyemail", "emailaddress"]),
            ("website", &["website", "companydomainname", "domain", "websiteurl", "web", "url"]),
            ("industry", &["industry"]),
            ("employees", &["numberofemployees", "employees", "employeecount"]),
            ("annual_revenue", &["annualrevenue", "revenue"]),
            ("owner", &["companyowner", "owner", "ownerid", "accountowner", "organizationowner", "assignedto", "accountownerid"]),
            ("description", &["description", "about", "aboutus", "notes"]),
            ("created_at", &["createdate", "createddate", "createdtime", "addtime", "createdat"]),
        ];
        v.extend(ADDRESS);
        v
    });
    static CONTACT_F: std::sync::LazyLock<Vec<(&str, &[&str])>> = std::sync::LazyLock::new(|| {
        let mut v: Vec<(&str, &[&str])> = vec![
            ("id", &["recordid", "id", "contactid", "personid"]),
            ("first_name", &["firstname", "givenname"]),
            ("last_name", &["lastname", "surname", "familyname"]),
            ("full_name", &["name", "fullname", "contactname", "personname"]),
            ("job_title", &["jobtitle", "title", "position"]),
            ("email", &["email", "emailaddress", "workemail", "personemail", "secondaryemail", "otheremail", "email2", "additionalemailaddresses", "emailwork", "emailhome", "emailother"]),
            ("phone", &["phone", "phonenumber", "workphone", "businessphone", "mobile", "mobilephone", "mobilephonenumber", "homephone", "otherphone", "phonework", "phonemobile", "phonehome", "phoneother"]),
            ("company_ref", &["associatedcompanyids", "associatedcompanyid", "accountid", "organizationid", "accountnameid", "primaryassociatedcompanyid"]),
            ("company_name", &["companyname", "company", "accountname", "organization", "associatedcompany", "organizationname"]),
            ("owner", &["contactowner", "owner", "ownerid", "personowner", "assignedto", "contactownerid"]),
            ("birthdate", &["birthdate", "dateofbirth", "birthday"]),
            ("source", &["leadsource", "source", "originalsource", "originalsourcetype"]),
            ("description", &["description", "notes", "background"]),
            ("created_at", &["createdate", "createddate", "createdtime", "addtime", "createdat"]),
        ];
        v.extend(ADDRESS);
        v
    });
    static LEAD_F: std::sync::LazyLock<Vec<(&str, &[&str])>> = std::sync::LazyLock::new(|| {
        let mut v: Vec<(&str, &[&str])> = vec![
            ("id", &["recordid", "id", "leadid"]),
            ("title", &["leadtitle", "leadname", "subject"]),
            ("first_name", &["firstname", "givenname"]),
            ("last_name", &["lastname", "surname"]),
            ("full_name", &["name", "fullname", "personname", "contactname"]),
            ("job_title", &["jobtitle", "title", "designation", "position"]),
            ("company_name", &["company", "companyname", "organization", "accountname", "organizationname"]),
            ("email", &["email", "emailaddress", "secondaryemail"]),
            ("phone", &["phone", "phonenumber", "mobile", "mobilephone", "workphone"]),
            ("status", &["status", "leadstatus", "stage", "leadstage"]),
            ("source", &["leadsource", "source"]),
            ("owner", &["leadowner", "owner", "ownerid", "assignedto", "leadownerid"]),
            ("description", &["description", "notes"]),
            ("created_at", &["createdate", "createddate", "createdtime", "addtime", "createdat"]),
        ];
        v.extend(ADDRESS);
        v
    });
    const DEAL_F: &[(&str, &[&str])] = &[
        ("id", &["recordid", "id", "dealid", "opportunityid"]),
        ("title", &["dealname", "name", "title", "opportunityname", "dealtitle"]),
        ("amount", &["amount", "value", "dealvalue", "opportunityamount", "dealamount", "expectedrevenue"]),
        ("currency", &["currency", "currencyisocode", "dealcurrency", "currencycode", "valuecurrency"]),
        ("pipeline", &["pipeline", "dealpipeline", "pipelinename"]),
        ("stage", &["dealstage", "stage", "stagename"]),
        ("close_date", &["closedate", "expectedclosedate", "closingdate", "closedat", "wontime"]),
        ("probability", &["probability", "dealprobability", "probabilitypercent"]),
        ("company_ref", &["associatedcompanyids", "associatedcompanyid", "accountid", "organizationid", "accountnameid"]),
        ("company_name", &["accountname", "organization", "organizationname", "companyname", "company", "associatedcompany"]),
        ("contact_ref", &["associatedcontactids", "associatedcontactid", "contactid", "contactnameid", "personid", "primarycontactid"]),
        ("contact_name", &["contactname", "contactperson", "person", "associatedcontact"]),
        ("owner", &["dealowner", "owner", "ownerid", "assignedto", "opportunityowner", "dealownerid"]),
        ("source", &["leadsource", "source", "dealsource"]),
        ("description", &["description", "dealdescription", "nextstep"]),
        ("created_at", &["createdate", "createddate", "createdtime", "addtime", "createdat", "dealcreated"]),
    ];
    const ACTIVITY_F: &[(&str, &[&str])] = &[
        ("id", &["recordid", "id", "activityid", "taskid", "eventid"]),
        ("type", &["type", "activitytype", "tasksubtype", "eventtype", "eventsubtype", "tasktype"]),
        ("subject", &["subject", "title", "activitysubject", "tasktitle", "name", "callsubject"]),
        ("description", &["description", "notes", "note", "body", "comments", "taskbody"]),
        ("due", &["duedate", "activitydate", "due", "date", "startdatetime", "duedatetime", "activityduedate", "duetime", "startdate"]),
        ("done", &["status", "done", "completed", "isclosed", "taskstatus"]),
        ("parent_ref", &["whatid", "whoid", "dealid", "personid", "organizationid", "leadid", "associateddealids", "associatedcontactids", "associatedcompanyids", "relatedtoid", "regardingid", "seid", "whatidid"]),
        ("owner", &["owner", "ownerid", "assignedto", "activityowner", "taskowner", "assignedtouser", "user"]),
    ];
    const NOTE_F: &[(&str, &[&str])] = &[
        ("id", &["recordid", "id", "noteid"]),
        ("title", &["title", "notetitle", "subject"]),
        ("body", &["body", "notebody", "content", "note", "text", "notecontent", "description"]),
        ("parent_ref", &["parentid", "dealid", "personid", "organizationid", "leadid", "associateddealids", "associatedcontactids", "associatedcompanyids", "whatid", "parentidid", "relatedtoid"]),
        ("owner", &["owner", "ownerid", "createdby", "createdbyid", "user", "noteowner"]),
        ("created_at", &["createddate", "createdat", "addtime", "createdtime", "createdate", "date"]),
    ];
    const USER_F: &[(&str, &[&str])] = &[
        ("id", &["id", "userid", "recordid"]),
        ("email", &["email", "emailaddress", "username"]),
        ("first_name", &["firstname"]),
        ("last_name", &["lastname"]),
        ("full_name", &["name", "fullname"]),
    ];
    match kind {
        "company" => &COMPANY_F,
        "contact" => &CONTACT_F,
        "lead" => &LEAD_F,
        "deal" => DEAL_F,
        "activity" => ACTIVITY_F,
        "note" => NOTE_F,
        "user" => USER_F,
        _ => &[],
    }
}

/// The fields a file of this kind can fill (for the mapping lists).
fn fields_of(kind: &str) -> Vec<&'static str> {
    synonyms(kind).iter().map(|(f, _)| *f).collect()
}

/// A file's kind from its name, else from its columns.
fn kind_of(t: &Table) -> &'static str {
    let n = t.name.to_lowercase();
    let n = n.trim_end_matches(".csv");
    let by_name = [
        (&["user"][..], "user"),
        (&["note"][..], "note"),
        (&["task", "activit", "event", "call", "meeting"][..], "activity"),
        (&["opportunit", "deal"][..], "deal"),
        (&["lead"][..], "lead"),
        (&["contact", "person", "people"][..], "contact"),
        (&["account", "compan", "organi"][..], "company"),
    ];
    for (words, kind) in by_name {
        if words.iter().any(|w| n.contains(w)) {
            return kind;
        }
    }
    let h: Vec<String> = t.headers.iter().map(|h| norm(h).0).collect();
    let has = |k: &str| h.iter().any(|x| x == k || x.starts_with(k));
    if has("deal") || has("opportunity") || has("stagename") {
        "deal"
    } else if has("lead") {
        "lead"
    } else if has("person") || has("firstname") {
        "contact"
    } else if has("organization") || has("companyname") || has("accountname") {
        "company"
    } else {
        "contact"
    }
}

/// Which system made the export, from its column names.
fn source_of(tables: &[Table]) -> &'static str {
    let all: Vec<String> = tables.iter().flat_map(|t| t.headers.iter().map(|h| h.to_lowercase())).collect();
    let has = |s: &str| all.iter().any(|h| h == s);
    if all.iter().any(|h| h.starts_with("organization - ") || h.starts_with("person - ") || h.starts_with("deal - ")) {
        "pipedrive"
    } else if has("record id") && (has("contact owner") || has("deal owner") || has("company owner") || has("deal stage") || has("associated company ids")) {
        "hubspot"
    } else if has("ownerid") || has("accountid") || has("stagename") || has("isdeleted") {
        "salesforce"
    } else if has("record id") || all.iter().any(|h| h.ends_with(" owner") || h == "account name.id") {
        "zoho"
    } else {
        "other"
    }
}

/// Column → field, by the synonyms; each field once (the first column that fits).
fn auto_mapping(kind: &str, headers: &[String]) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut taken: Vec<&str> = Vec::new();
    // Exact matches on the full name first (so "Company name" isn't taken by "Name").
    for pass in 0..2 {
        for h in headers {
            if out.contains_key(h) {
                continue;
            }
            let (full, short) = norm(h);
            for (field, names) in synonyms(kind) {
                // Several columns may fill a list (emails, phones) or be tried in turn (links).
                let multi = matches!(*field, "email" | "phone" | "parent_ref" | "contact_ref");
                if !multi && taken.contains(field) {
                    continue;
                }
                let hit = if pass == 0 { names.contains(&full.as_str()) } else { names.contains(&short.as_str()) };
                if hit {
                    out.insert(h.clone(), field.to_string());
                    taken.push(field);
                    break;
                }
            }
        }
    }
    out
}

fn col(t: &Table, mapping: &BTreeMap<String, String>, field: &str) -> Vec<usize> {
    t.headers.iter().enumerate().filter(|(_, h)| mapping.get(*h).map(String::as_str) == Some(field)).map(|(i, _)| i).collect()
}

fn val<'a>(row: &'a [String], cols: &[usize]) -> Option<&'a str> {
    cols.iter().filter_map(|i| row.get(*i)).map(String::as_str).find(|v| !v.is_empty())
}

fn semantics_of(stage: &str) -> &'static str {
    let s = stage.to_lowercase();
    if s.contains("won") || s == "closed - won" || s.contains("converted") {
        "success"
    } else if s.contains("lost") || s.contains("junk") || s.contains("unqualified") || s.contains("not qualified") || s.contains("disqualified") || s.contains("dead") {
        "failure"
    } else {
        "process"
    }
}

// ---------------------------------------------------------------------------
// Analysis
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct AnalyzeInput {
    file_ids: Vec<Uuid>,
    #[serde(default)]
    source: Option<String>,
}

/// Employees (active, not guests): id, email, full name.
async fn employees(state: &AppState) -> AppResult<Vec<(Uuid, String, String)>> {
    Ok(sqlx::query_as("SELECT id, lower(email), lower(trim(first_name || ' ' || last_name)) FROM users WHERE is_active AND NOT is_guest").fetch_all(&state.db).await?)
}

/// An owner value from the export → an employee: by the export's users file, email, or name.
fn match_owner(value: &str, users_file: &HashMap<String, String>, staff: &[(Uuid, String, String)]) -> Option<Uuid> {
    let v = users_file.get(value).map(String::as_str).unwrap_or(value).trim().to_lowercase();
    staff.iter().find(|(_, e, n)| *e == v || *n == v).map(|(id, _, _)| *id)
}

/// Salesforce-style users files: their id → email (or name).
fn users_file(tables: &[(String, &Table, BTreeMap<String, String>)]) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for (kind, t, m) in tables {
        if kind != "user" {
            continue;
        }
        let (id, email, first, last, full) = (col(t, m, "id"), col(t, m, "email"), col(t, m, "first_name"), col(t, m, "last_name"), col(t, m, "full_name"));
        for r in &t.rows {
            let Some(i) = val(r, &id) else { continue };
            let who = val(r, &email).map(String::from).or_else(|| val(r, &full).map(String::from)).or_else(|| {
                let n = format!("{} {}", val(r, &first).unwrap_or(""), val(r, &last).unwrap_or(""));
                (!n.trim().is_empty()).then(|| n.trim().to_string())
            });
            if let Some(w) = who {
                out.insert(i.to_string(), w);
            }
        }
    }
    out
}

async fn analyze(State(state): State<AppState>, auth: AuthUser, Json(b): Json<AnalyzeInput>) -> AppResult<Json<Value>> {
    require_admin(&auth)?;
    if b.file_ids.is_empty() || b.file_ids.len() > 20 {
        return Err(AppError::bad("upload 1 to 20 files"));
    }
    let tables = tables(&state, &auth, &b.file_ids).await?;
    let source = match b.source.as_deref() {
        Some(s) if SOURCES.contains(&s) => s,
        _ => source_of(&tables),
    };
    let mapped: Vec<(String, &Table, BTreeMap<String, String>)> = tables.iter().map(|t| {
        let k = kind_of(t);
        (k.to_string(), t, auto_mapping(k, &t.headers))
    }).collect();
    let users = users_file(&mapped);
    let staff = employees(&state).await?;

    let mut pipelines: BTreeMap<String, i64> = BTreeMap::new();
    let mut stages: BTreeMap<(String, String, String), i64> = BTreeMap::new();
    let mut owners: BTreeMap<String, i64> = BTreeMap::new();
    let mut currencies: BTreeMap<String, i64> = BTreeMap::new();
    for (kind, t, m) in &mapped {
        let (pipe, stage, status, owner, cur) = (col(t, m, "pipeline"), col(t, m, "stage"), col(t, m, "status"), col(t, m, "owner"), col(t, m, "currency"));
        for r in &t.rows {
            if let Some(o) = val(r, &owner) {
                *owners.entry(o.to_string()).or_default() += 1;
            }
            match kind.as_str() {
                "deal" => {
                    let p = val(r, &pipe).unwrap_or("").to_string();
                    *pipelines.entry(p.clone()).or_default() += 1;
                    if let Some(s) = val(r, &stage) {
                        *stages.entry(("deal".into(), p, s.to_string())).or_default() += 1;
                    }
                    if let Some(c) = val(r, &cur) {
                        *currencies.entry(c.to_uppercase()).or_default() += 1;
                    }
                }
                "lead" => {
                    if let Some(s) = val(r, &status) {
                        *stages.entry(("lead".into(), String::new(), s.to_string())).or_default() += 1;
                    }
                }
                _ => {}
            }
        }
    }
    // Suggestions: pipelines and stages by name.
    let cats: Vec<(Uuid, String, bool)> = sqlx::query_as("SELECT id, name, is_default FROM crm_categories WHERE entity_type = $1 ORDER BY sort").bind(DEAL).fetch_all(&state.db).await?;
    let default_cat = cats.iter().find(|c| c.2).map(|c| c.0);
    let pipe_for = |p: &str| -> Value {
        if p.is_empty() {
            return json!(default_cat);
        }
        cats.iter().find(|c| c.1.eq_ignore_ascii_case(p)).map(|c| json!(c.0)).unwrap_or(json!("new"))
    };
    let all_stages: Vec<(Uuid, Uuid, i16, String, String)> = sqlx::query_as(
        "SELECT s.id, s.category_id, c.entity_type, s.name, s.semantics FROM crm_stages s JOIN crm_categories c ON c.id = s.category_id WHERE c.entity_type IN (1, 2) ORDER BY s.sort",
    )
    .fetch_all(&state.db)
    .await?;
    let lead_cat: Option<Uuid> = all_stages.iter().find(|s| s.2 == LEAD).map(|s| s.1);
    let stage_rows: Vec<Value> = stages
        .iter()
        .map(|((kind, pipe, value), n)| {
            let sem = semantics_of(value);
            let cat = if kind == "lead" { lead_cat.map(|c| json!(c)).unwrap_or(Value::Null) } else { pipe_for(pipe) };
            let cat_id = cat.as_str().and_then(|c| Uuid::parse_str(c).ok());
            let suggestion = cat_id.and_then(|c| {
                let here: Vec<&(Uuid, Uuid, i16, String, String)> = all_stages.iter().filter(|s| s.1 == c).collect();
                here.iter().find(|s| s.3.eq_ignore_ascii_case(value)).or_else(|| (sem != "process").then(|| here.iter().find(|s| s.4 == sem)).flatten()).map(|s| s.0)
            });
            json!({
                "kind": kind, "pipeline": pipe, "value": value, "count": n, "semantics": sem,
                "stage_id": suggestion.map(|s| json!(s)).unwrap_or(if kind == "deal" { json!("new") } else { Value::Null }),
            })
        })
        .collect();
    let enabled: Vec<String> = sqlx::query_scalar("SELECT code FROM crm_currencies").fetch_all(&state.db).await?;
    Ok(Json(json!({
        "source": source,
        "files": mapped.iter().map(|(kind, t, m)| json!({
            "name": t.name, "kind": kind, "rows": t.rows.len(), "columns": t.headers, "mapping": m,
            "samples": t.rows.iter().take(3).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "fields": KINDS.iter().map(|k| (k.to_string(), json!(fields_of(k)))).collect::<Map<String, Value>>(),
        "pipelines": pipelines.iter().map(|(p, n)| json!({ "value": p, "count": n, "category_id": pipe_for(p) })).collect::<Vec<_>>(),
        "stages": stage_rows,
        "owners": owners.iter().map(|(o, n)| json!({
            "value": o, "count": n, "via": users.get(o), "user_id": match_owner(o, &users, &staff),
        })).collect::<Vec<_>>(),
        "currencies": currencies.iter().map(|(c, n)| json!({ "code": c, "count": n, "enabled": enabled.iter().any(|e| e.trim() == c) })).collect::<Vec<_>>(),
    })))
}

// ---------------------------------------------------------------------------
// Starting, listing, undoing
// ---------------------------------------------------------------------------

#[derive(Deserialize, serde::Serialize, Clone)]
struct PlanFile {
    name: String,
    /// company | contact | lead | deal | activity | note | user | skip
    kind: String,
    mapping: BTreeMap<String, String>,
}

#[derive(Deserialize, serde::Serialize, Clone)]
struct Plan {
    file_ids: Vec<Uuid>,
    source: String,
    files: Vec<PlanFile>,
    /// Deal pipeline value → pipeline id, or "new" (made with the stages found).
    #[serde(default)]
    pipelines: BTreeMap<String, String>,
    /// "deal|<pipeline>|<stage>" or "lead||<status>" → stage id, or "new".
    #[serde(default)]
    stages: BTreeMap<String, String>,
    /// Owner value → employee.
    #[serde(default)]
    owners: BTreeMap<String, Uuid>,
    /// update | skip: records brought over before (or matching existing ones).
    #[serde(default = "update")]
    duplicates: String,
}
fn update() -> String {
    "update".into()
}

async fn start(State(state): State<AppState>, auth: AuthUser, Json(p): Json<Plan>) -> AppResult<Json<Value>> {
    require_admin(&auth)?;
    if !SOURCES.contains(&p.source.as_str()) {
        return Err(AppError::bad("unknown source"));
    }
    if !["update", "skip"].contains(&p.duplicates.as_str()) {
        return Err(AppError::bad("duplicates must be update or skip"));
    }
    let names: Vec<String> = tables(&state, &auth, &p.file_ids).await?.into_iter().map(|t| t.name).collect();
    for f in &p.files {
        if !names.contains(&f.name) {
            return Err(AppError::bad(format!("{} is not in the upload", f.name)));
        }
        if f.kind != "skip" && !KINDS.contains(&f.kind.as_str()) {
            return Err(AppError::bad(format!("{}: unknown kind {}", f.name, f.kind)));
        }
        let fields = fields_of(&f.kind);
        if let Some((c, bad)) = f.mapping.iter().find(|(_, v)| !v.is_empty() && v.as_str() != "skip" && !fields.contains(&v.as_str())) {
            return Err(AppError::bad(format!("{}: column {c} can't go to {bad}", f.name)));
        }
    }
    for v in p.stages.values().chain(p.pipelines.values()) {
        if v != "new" && Uuid::parse_str(v).is_err() {
            return Err(AppError::bad("stages and pipelines map to an id or \"new\""));
        }
    }
    let id: Uuid = sqlx::query_scalar("INSERT INTO crm_migrations (source, plan, created_by) VALUES ($1, $2, $3) RETURNING id")
        .bind(&p.source)
        .bind(json!(p))
        .bind(auth.id)
        .fetch_one(&state.db)
        .await?;
    jobs::enqueue(&state.db, JOB, json!({ "migration_id": id }), None).await?;
    state.job_wakeup.notify_one();
    Ok(Json(load(&state, id).await?))
}

async fn load(state: &AppState, id: Uuid) -> AppResult<Value> {
    let row: Option<Value> = sqlx::query_scalar(
        "SELECT jsonb_build_object('id', m.id, 'source', m.source, 'status', m.status, 'progress', m.progress, 'created_at', m.created_at,
                'finished_at', m.finished_at, 'undone_at', m.undone_at, 'files', (SELECT jsonb_agg(f->>'name') FROM jsonb_array_elements(m.plan->'files') f),
                'created_by', m.created_by, 'created_by_name', trim(u.first_name || ' ' || u.last_name))
         FROM crm_migrations m LEFT JOIN users u ON u.id = m.created_by WHERE m.id = $1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?;
    row.ok_or(AppError::NotFound("migration"))
}

async fn one(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    require_admin(&auth)?;
    Ok(Json(load(&state, id).await?))
}

async fn list(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    require_admin(&auth)?;
    let ids: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM crm_migrations ORDER BY created_at DESC LIMIT 50").fetch_all(&state.db).await?;
    let mut items = Vec::new();
    for id in ids {
        items.push(load(&state, id).await?);
    }
    Ok(Json(json!({ "items": items })))
}

async fn undo(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    require_admin(&auth)?;
    let status: Option<String> = sqlx::query_scalar("SELECT status FROM crm_migrations WHERE id = $1").bind(id).fetch_optional(&state.db).await?;
    match status.as_deref() {
        None => return Err(AppError::NotFound("migration")),
        Some("queued") | Some("running") => return Err(AppError::Conflict("the migration is still running".into())),
        Some("undone") => return Err(AppError::Conflict("already undone".into())),
        _ => {}
    }
    let mut tx = state.db.begin().await?;
    let records = sqlx::query(
        "UPDATE crm_items SET deleted_at = now(), deleted_by = $2 WHERE deleted_at IS NULL AND id IN
           (SELECT item_id FROM crm_migration_ids WHERE migration_id = $1 AND created AND kind IN ('company', 'contact', 'lead', 'deal'))",
    )
    .bind(id)
    .bind(auth.id)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    let activities = sqlx::query("DELETE FROM crm_activities WHERE id IN (SELECT item_id FROM crm_migration_ids WHERE migration_id = $1 AND created AND kind = 'activity')")
        .bind(id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    let notes = sqlx::query("DELETE FROM crm_timeline WHERE id IN (SELECT item_id FROM crm_migration_ids WHERE migration_id = $1 AND created AND kind = 'note')")
        .bind(id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    sqlx::query("DELETE FROM crm_migration_ids WHERE migration_id = $1").bind(id).execute(&mut *tx).await?;
    sqlx::query("UPDATE crm_migrations SET status = 'undone', undone_at = now() WHERE id = $1").bind(id).execute(&mut *tx).await?;
    tx.commit().await?;
    crate::audit::record(&state.db, Some(auth.id), "crm.migration_undo", "crm_migration", Some(id.to_string()), json!({ "records": records, "activities": activities, "notes": notes }), None).await;
    Ok(Json(json!({ "records": records, "activities": activities, "notes": notes })))
}

// ---------------------------------------------------------------------------
// Running
// ---------------------------------------------------------------------------

fn parse_when(raw: &str) -> Option<DateTime<Utc>> {
    let raw = raw.trim();
    if let Ok(d) = DateTime::parse_from_rfc3339(raw) {
        return Some(d.with_timezone(&Utc));
    }
    // Epoch milliseconds (some APIs' exports).
    if raw.len() == 13 && raw.chars().all(|c| c.is_ascii_digit()) {
        return raw.parse::<i64>().ok().and_then(DateTime::from_timestamp_millis);
    }
    for f in ["%Y-%m-%d %H:%M:%S", "%Y-%m-%d %H:%M", "%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%dT%H:%M:%S", "%m/%d/%Y %H:%M", "%m/%d/%Y %I:%M %p", "%d.%m.%Y %H:%M", "%d/%m/%Y %H:%M", "%b %d, %Y %I:%M %p"] {
        if let Ok(t) = NaiveDateTime::parse_from_str(raw, f) {
            return Some(t.and_utc());
        }
    }
    for f in ["%Y-%m-%d", "%m/%d/%Y", "%d.%m.%Y", "%Y/%m/%d", "%b %d, %Y", "%d %b %Y"] {
        if let Ok(d) = NaiveDate::parse_from_str(raw, f) {
            return d.and_hms_opt(0, 0, 0).map(|t| t.and_utc());
        }
    }
    None
}

#[derive(Default)]
struct Counts(BTreeMap<String, [u64; 4]>);
impl Counts {
    fn add(&mut self, kind: &str, i: usize) {
        self.0.entry(kind.to_string()).or_default()[i] += 1;
    }
    fn json(&self) -> Value {
        self.0.iter().map(|(k, c)| (k.clone(), json!({ "created": c[0], "updated": c[1], "skipped": c[2], "failed": c[3] }))).collect::<Map<String, Value>>().into()
    }
}

struct Run<'a> {
    state: &'a AppState,
    auth: AuthUser,
    id: Uuid,
    plan: Plan,
    users: HashMap<String, String>,
    staff: Vec<(Uuid, String, String)>,
    /// Pipeline value → pipeline (made as needed).
    cats: HashMap<String, Uuid>,
    /// "kind|pipeline|value" → stage (made as needed).
    stage_ids: HashMap<String, Uuid>,
    currencies: Vec<String>,
}

impl Run<'_> {
    async fn known(&self, kind: &str, source_id: &str) -> Result<Option<Uuid>> {
        let id: Option<Uuid> = sqlx::query_scalar("SELECT item_id FROM crm_migration_ids WHERE source = $1 AND kind = $2 AND source_id = $3")
            .bind(&self.plan.source)
            .bind(kind)
            .bind(source_id)
            .fetch_optional(&self.state.db)
            .await?;
        // Records deleted since don't count.
        Ok(match (id, kind) {
            (Some(i), "company" | "contact" | "lead" | "deal") => {
                let alive: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM crm_items WHERE id = $1 AND deleted_at IS NULL)").bind(i).fetch_one(&self.state.db).await?;
                alive.then_some(i)
            }
            (Some(i), "activity") => sqlx::query_scalar("SELECT id FROM crm_activities WHERE id = $1").bind(i).fetch_optional(&self.state.db).await?,
            (Some(i), "note") => sqlx::query_scalar("SELECT id FROM crm_timeline WHERE id = $1").bind(i).fetch_optional(&self.state.db).await?,
            _ => None,
        })
    }

    async fn remember(&self, kind: &str, source_id: Option<&str>, item: Uuid, created: bool) -> Result<()> {
        let Some(sid) = source_id else { return Ok(()) };
        sqlx::query(
            "INSERT INTO crm_migration_ids (source, kind, source_id, item_id, migration_id, created) VALUES ($1, $2, $3, $4, $5, $6)
             ON CONFLICT (source, kind, source_id) DO UPDATE SET
               -- The same record seen again: it stays with the migration that made it.
               migration_id = CASE WHEN crm_migration_ids.item_id = EXCLUDED.item_id AND crm_migration_ids.created THEN crm_migration_ids.migration_id ELSE EXCLUDED.migration_id END,
               created = CASE WHEN crm_migration_ids.item_id = EXCLUDED.item_id THEN crm_migration_ids.created OR EXCLUDED.created ELSE EXCLUDED.created END,
               item_id = EXCLUDED.item_id",
        )
        .bind(&self.plan.source)
        .bind(kind)
        .bind(sid)
        .bind(item)
        .bind(self.id)
        .bind(created)
        .execute(&self.state.db)
        .await?;
        Ok(())
    }

    fn owner(&self, value: Option<&str>) -> Option<Uuid> {
        let v = value?;
        self.plan.owners.get(v).copied().or_else(|| match_owner(v, &self.users, &self.staff))
    }

    /// The record an id (or several, "1;2") of the old system points to, in these kinds.
    async fn find_ref(&self, kinds: &[&str], raw: &str) -> Result<Option<(String, Uuid)>> {
        for part in raw.split([';', ',', '|']).map(str::trim).filter(|p| !p.is_empty()) {
            for k in kinds {
                if let Some(i) = self.known(k, part).await? {
                    return Ok(Some((k.to_string(), i)));
                }
            }
        }
        Ok(None)
    }

    async fn company_named(&self, name: &str) -> Result<Uuid> {
        let found: Option<Uuid> = sqlx::query_scalar("SELECT id FROM crm_items WHERE entity_type = 4 AND deleted_at IS NULL AND lower(title) = lower($1) ORDER BY created_at LIMIT 1")
            .bind(name)
            .fetch_optional(&self.state.db)
            .await?;
        if let Some(f) = found {
            return Ok(f);
        }
        let t = entity_type_by_id(self.state, COMPANY).await?;
        let mut body = Map::new();
        body.insert("company_name".into(), json!(name));
        Ok(import_item(self.state, &self.auth, &t, None, body, None, json!({ "imported_from": self.plan.source })).await.map_err(|e| anyhow!(e.to_string()))?)
    }

    /// The pipeline a deal pipeline value goes to (making it when the plan says "new").
    async fn category(&mut self, pipeline: &str) -> Result<Option<Uuid>> {
        if let Some(c) = self.cats.get(pipeline) {
            return Ok(Some(*c));
        }
        let choice = self.plan.pipelines.get(pipeline).cloned().unwrap_or_default();
        let id = if choice == "new" {
            let name = if pipeline.is_empty() { format!("From {}", self.plan.source) } else { pipeline.chars().take(100).collect() };
            let existing: Option<Uuid> = sqlx::query_scalar("SELECT id FROM crm_categories WHERE entity_type = $1 AND lower(name) = lower($2)").bind(DEAL).bind(&name).fetch_optional(&self.state.db).await?;
            match existing {
                Some(e) => e,
                None => {
                    let c: Uuid = sqlx::query_scalar(
                        "INSERT INTO crm_categories (entity_type, name, sort) VALUES ($1, $2, COALESCE((SELECT MAX(sort) FROM crm_categories WHERE entity_type = $1), 0) + 10) RETURNING id",
                    )
                    .bind(DEAL)
                    .bind(&name)
                    .fetch_one(&self.state.db)
                    .await?;
                    // Its stages: the ones the export uses there, in the order first seen;
                    // closing stages at the end, and a won and a lost one at least.
                    let mut seen: Vec<String> = Vec::new();
                    for k in self.plan.stages.keys() {
                        let mut it = k.splitn(3, '|');
                        if let (Some("deal"), Some(p), Some(s)) = (it.next(), it.next(), it.next())
                            && p == pipeline && !seen.iter().any(|x| x.eq_ignore_ascii_case(s)) {
                                seen.push(s.to_string());
                            }
                    }
                    let mut ordered: Vec<(String, &str)> = seen.iter().map(|s| (s.clone(), semantics_of(s))).collect();
                    ordered.sort_by_key(|(_, sem)| match *sem { "process" => 0, "success" => 1, _ => 2 });
                    if !ordered.iter().any(|(_, s)| *s == "process") {
                        ordered.insert(0, ("New".into(), "process"));
                    }
                    if !ordered.iter().any(|(_, s)| *s == "success") {
                        ordered.push(("Won".into(), "success"));
                    }
                    if !ordered.iter().any(|(_, s)| *s == "failure") {
                        ordered.push(("Lost".into(), "failure"));
                    }
                    for (i, (n, sem)) in ordered.iter().enumerate() {
                        let color = match *sem { "success" => "#22c55e", "failure" => "#ef4444", _ => "#3b82f6" };
                        sqlx::query("INSERT INTO crm_stages (category_id, name, color, sort, semantics) VALUES ($1, $2, $3, $4, $5)")
                            .bind(c)
                            .bind(n.chars().take(60).collect::<String>())
                            .bind(color)
                            .bind((i as i32 + 1) * 10)
                            .bind(sem)
                            .execute(&self.state.db)
                            .await?;
                    }
                    c
                }
            }
        } else if let Ok(u) = Uuid::parse_str(&choice) {
            u
        } else {
            return Ok(None);
        };
        self.cats.insert(pipeline.to_string(), id);
        Ok(Some(id))
    }

    /// The stage a deal stage (in its pipeline) or a lead status goes to.
    async fn stage(&mut self, kind: &str, pipeline: &str, value: &str) -> Result<Option<Uuid>> {
        let key = format!("{kind}|{pipeline}|{value}");
        if let Some(s) = self.stage_ids.get(&key) {
            return Ok(Some(*s));
        }
        let choice = self.plan.stages.get(&key).cloned().unwrap_or_default();
        let cat = if kind == "deal" {
            self.category(pipeline).await?
        } else {
            sqlx::query_scalar("SELECT id FROM crm_categories WHERE entity_type = $1 ORDER BY is_default DESC, sort LIMIT 1").bind(LEAD).fetch_optional(&self.state.db).await?
        };
        let id = if let Ok(u) = Uuid::parse_str(&choice) {
            Some(u)
        } else if let Some(c) = cat {
            // "new", or nothing chosen: the stage of that name in the pipeline, else a new one.
            let found: Option<Uuid> = sqlx::query_scalar("SELECT id FROM crm_stages WHERE category_id = $1 AND lower(name) = lower($2)").bind(c).bind(value).fetch_optional(&self.state.db).await?;
            match found {
                Some(f) => Some(f),
                None if choice == "new" => {
                    let sem = semantics_of(value);
                    Some(
                        sqlx::query_scalar(
                            "INSERT INTO crm_stages (category_id, name, color, semantics, sort)
                             VALUES ($1, $2, $3, $4, COALESCE((SELECT MAX(sort) FROM crm_stages WHERE category_id = $1 AND (semantics = 'process' OR $4 <> 'process')), 0) + 5) RETURNING id",
                        )
                        .bind(c)
                        .bind(value.chars().take(60).collect::<String>())
                        .bind(match sem { "success" => "#22c55e", "failure" => "#ef4444", _ => "#3b82f6" })
                        .bind(sem)
                        .fetch_one(&self.state.db)
                        .await?,
                    )
                }
                None => None,
            }
        } else {
            None
        };
        if let Some(i) = id {
            self.stage_ids.insert(key, i);
        }
        Ok(id)
    }
}

/// A person's name parts: from first/last, or a full name split at its last space.
fn name_parts(first: Option<&str>, last: Option<&str>, full: Option<&str>) -> (Option<String>, Option<String>) {
    if first.is_some() || last.is_some() {
        return (first.map(String::from), last.map(String::from));
    }
    match full.map(str::trim).filter(|f| !f.is_empty()) {
        Some(f) => match f.rsplit_once(' ') {
            Some((a, b)) => (Some(a.trim().to_string()), Some(b.trim().to_string())),
            None => (None, Some(f.to_string())),
        },
        None => (None, None),
    }
}

/// HTML (HubSpot and Zoho notes) as plain text.
fn plain(text: &str) -> String {
    if text.contains('<') && text.contains('>') {
        htmd::convert(text).unwrap_or_else(|_| text.to_string())
    } else {
        text.to_string()
    }
}

pub async fn run_job(state: &AppState, payload: &Value) -> Result<()> {
    let id: Uuid = serde_json::from_value(payload["migration_id"].clone())?;
    let (plan, progress, created_by, status): (Value, Value, Option<Uuid>, String) =
        sqlx::query_as("SELECT plan, progress, created_by, status FROM crm_migrations WHERE id = $1").bind(id).fetch_one(&state.db).await?;
    if !matches!(status.as_str(), "queued" | "running") {
        return Ok(());
    }
    let fail = |msg: String| async move {
        sqlx::query("UPDATE crm_migrations SET status = 'failed', finished_at = now(), progress = progress || jsonb_build_object('error', $2::text) WHERE id = $1")
            .bind(id)
            .bind(msg)
            .execute(&state.db)
            .await
    };
    let Some(auth) = (match created_by {
        Some(u) => crate::auth::acting_as(state, u).await.map_err(|e| anyhow!(e.to_string()))?,
        None => None,
    }) else {
        fail("the person who started it is no longer active".into()).await?;
        return Ok(());
    };
    let plan: Plan = serde_json::from_value(plan)?;
    let all = match tables(state, &auth, &plan.file_ids).await {
        Ok(t) => t,
        Err(e) => {
            fail(e.to_string()).await?;
            return Ok(());
        }
    };
    sqlx::query("UPDATE crm_migrations SET status = 'running' WHERE id = $1").bind(id).execute(&state.db).await?;

    // Files in the order of their kinds.
    let mut work: Vec<(PlanFile, &Table)> = Vec::new();
    for k in KINDS {
        for f in plan.files.iter().filter(|f| f.kind == *k) {
            if let Some(t) = all.iter().find(|t| t.name == f.name) {
                work.push((f.clone(), t));
            }
        }
    }
    let mapped: Vec<(String, &Table, BTreeMap<String, String>)> = work.iter().map(|(f, t)| (f.kind.clone(), *t, f.mapping.clone())).collect();
    let mut run = Run {
        state,
        users: users_file(&mapped),
        staff: employees(state).await.map_err(|e| anyhow!(e.to_string()))?,
        auth,
        id,
        plan: plan.clone(),
        cats: HashMap::new(),
        stage_ids: HashMap::new(),
        currencies: sqlx::query_scalar::<_, String>("SELECT trim(code) FROM crm_currencies").fetch_all(&state.db).await?,
    };

    let mut counts = Counts::default();
    if let Some(c) = progress.get("counts").and_then(Value::as_object) {
        for (k, v) in c {
            let e = counts.0.entry(k.clone()).or_default();
            for (i, f) in ["created", "updated", "skipped", "failed"].iter().enumerate() {
                e[i] = v[f].as_u64().unwrap_or(0);
            }
        }
    }
    let mut errors: Vec<Value> = progress.get("errors").and_then(Value::as_array).cloned().unwrap_or_default();
    let (mut fi, mut ri) = (progress["file"].as_u64().unwrap_or(0) as usize, progress["row"].as_u64().unwrap_or(0) as usize);
    let total: usize = work.iter().filter(|(f, _)| f.kind != "user").map(|(_, t)| t.rows.len()).sum();
    let mut done: usize = progress["done"].as_u64().unwrap_or(0) as usize;
    let started = std::time::Instant::now();
    let save = |fi: usize, ri: usize, done: usize, counts: &Counts, errors: &[Value]| {
        json!({ "file": fi, "row": ri, "done": done, "total": total, "counts": counts.json(), "errors": errors })
    };

    while fi < work.len() {
        let (f, t) = (&work[fi].0, work[fi].1);
        if f.kind == "user" {
            fi += 1;
            ri = 0;
            continue;
        }
        while ri < t.rows.len() {
            let row = &t.rows[ri];
            let outcome = one_row(&mut run, &f.kind, t, &f.mapping, row).await;
            match outcome {
                Ok(i) => counts.add(&f.kind, i),
                Err(e) => {
                    counts.add(&f.kind, 3);
                    if errors.len() < 200 {
                        errors.push(json!({ "file": t.name, "line": ri + 2, "error": e.to_string() }));
                    }
                }
            }
            ri += 1;
            done += 1;
            if done % 50 == 0 {
                sqlx::query("UPDATE crm_migrations SET progress = $2 WHERE id = $1").bind(id).bind(save(fi, ri, done, &counts, &errors)).execute(&state.db).await?;
            }
            // Jobs have a time limit: carry on in a fresh run.
            if started.elapsed() > std::time::Duration::from_secs(150) {
                sqlx::query("UPDATE crm_migrations SET progress = $2 WHERE id = $1").bind(id).bind(save(fi, ri, done, &counts, &errors)).execute(&state.db).await?;
                jobs::enqueue(&state.db, JOB, json!({ "migration_id": id }), None).await?;
                return Ok(());
            }
        }
        fi += 1;
        ri = 0;
    }
    sqlx::query("UPDATE crm_migrations SET status = 'done', finished_at = now(), progress = $2 WHERE id = $1").bind(id).bind(save(fi, 0, done, &counts, &errors)).execute(&state.db).await?;
    for t in [LEAD, DEAL, CONTACT, COMPANY] {
        super::broadcast(state, t, Uuid::nil(), false).await;
    }
    Ok(())
}

/// One row: 0 created, 1 updated, 2 skipped.
async fn one_row(run: &mut Run<'_>, kind: &str, t: &Table, m: &BTreeMap<String, String>, row: &[String]) -> Result<usize> {
    let get = |field: &str| val(row, &col(t, m, field)).map(String::from);
    let all = |field: &str| -> Vec<String> {
        col(t, m, field)
            .iter()
            .filter_map(|i| row.get(*i))
            .flat_map(|v| v.replace(":::", ";").split([';', ',']).map(|p| p.trim().to_string()).collect::<Vec<_>>())
            .filter(|v| !v.is_empty())
            .collect()
    };
    let source_id = get("id");
    let note = json!({ "imported_from": run.plan.source, "source_id": source_id });
    let created_at = get("created_at").as_deref().and_then(parse_when);
    let owner = run.owner(get("owner").as_deref());

    match kind {
        "activity" | "note" => {
            let parent = match get("parent_ref") {
                Some(_) => {
                    let mut found = None;
                    for c in col(t, m, "parent_ref") {
                        if let Some(v) = row.get(c).filter(|v| !v.is_empty())
                            && let Some(p) = run.find_ref(&["deal", "lead", "contact", "company"], v).await? {
                                found = Some(p.1);
                                break;
                            }
                    }
                    found
                }
                None => None,
            };
            let Some(item) = parent else {
                return Err(anyhow!("the record it belongs to wasn't brought over (or the column linking it isn't mapped)"));
            };
            if let Some(sid) = &source_id
                && let Some(existing) = run.known(kind, sid).await? {
                    if run.plan.duplicates == "skip" {
                        return Ok(2);
                    }
                    // Notes and activities are brought over once; a second run leaves them.
                    let _ = existing;
                    return Ok(2);
                }
            let who = owner.unwrap_or(run.auth.id);
            let new_id: Uuid = if kind == "note" {
                let body = [get("title"), get("body").map(|b| plain(&b))].into_iter().flatten().filter(|s| !s.trim().is_empty()).collect::<Vec<_>>().join("\n\n");
                if body.trim().is_empty() {
                    return Ok(2);
                }
                sqlx::query_scalar("INSERT INTO crm_timeline (item_id, kind, author_id, body, data, created_at) VALUES ($1, 'comment', $2, $3, $4, COALESCE($5, now())) RETURNING id")
                    .bind(item)
                    .bind(who)
                    .bind(body.chars().take(20_000).collect::<String>())
                    .bind(&note)
                    .bind(created_at)
                    .fetch_one(&run.state.db)
                    .await?
            } else {
                let ty = get("type").unwrap_or_default().to_lowercase();
                let akind = if ty.contains("call") { "call" } else if ty.contains("meet") || ty.contains("event") { "meeting" } else if ty.contains("email") || ty.contains("mail") { "email" } else { "todo" };
                let subject: String = get("subject").or_else(|| get("type")).unwrap_or_else(|| "Activity".into()).chars().take(500).collect();
                let due = get("due").as_deref().and_then(parse_when);
                let done = get("done").map(|d| {
                    let d = d.to_lowercase();
                    ["completed", "complete", "done", "true", "1", "yes", "closed", "held"].iter().any(|w| d == *w || d.starts_with(w))
                });
                // Without a status column: what is past is done.
                let done = done.unwrap_or_else(|| due.is_some_and(|d| d < Utc::now()));
                sqlx::query_scalar(
                    "INSERT INTO crm_activities (item_id, kind, subject, description, starts_at, deadline, responsible_id, created_by, is_completed, completed_at)
                     VALUES ($1, $2, $3, $4, CASE WHEN $2 = 'meeting' THEN $5 END, $5, $6, $7, $8, CASE WHEN $8 THEN COALESCE($5, now()) END) RETURNING id",
                )
                .bind(item)
                .bind(akind)
                .bind(&subject)
                .bind(get("description").map(|d| plain(&d).chars().take(20_000).collect::<String>()))
                .bind(due)
                .bind(who)
                .bind(run.auth.id)
                .bind(done)
                .fetch_one(&run.state.db)
                .await?
            };
            run.remember(kind, source_id.as_deref(), new_id, true).await?;
            Ok(0)
        }
        _ => {
            let entity = match kind {
                "company" => COMPANY,
                "contact" => CONTACT,
                "lead" => LEAD,
                "deal" => DEAL,
                _ => return Ok(2),
            };
            let et = entity_type_by_id(run.state, entity).await?;
            let mut body = Map::new();
            let mut multi: Vec<Value> = Vec::new();
            let mut keys: Vec<(String, String)> = Vec::new();
            for e in all("email") {
                keys.push(("email".into(), normalize("email", &e)));
                multi.push(json!({ "kind": "email", "value_type": "work", "value": e }));
            }
            for p in all("phone") {
                keys.push(("phone".into(), normalize("phone", &p)));
                multi.push(json!({ "kind": "phone", "value_type": "work", "value": p }));
            }
            if let Some(w) = get("website") {
                multi.push(json!({ "kind": "web", "value_type": "work", "value": w }));
            }
            let mut address = Map::new();
            for k in ["line1", "city", "region", "postal_code", "country"] {
                if let Some(v) = get(&format!("address.{k}")) {
                    if k == "country" {
                        if v.len() == 2 && v.chars().all(|c| c.is_ascii_alphabetic()) {
                            address.insert(k.into(), json!(v.to_uppercase()));
                        }
                    } else {
                        address.insert(k.into(), json!(v));
                    }
                }
            }
            let mut comments: Vec<String> = Vec::new();
            if let Some(d) = get("description") {
                comments.push(plain(&d));
            }
            for (field, label) in [("industry", "Industry"), ("employees", "Employees")] {
                if let Some(v) = get(field) {
                    comments.push(format!("{label}: {v}"));
                }
            }
            if let Some(s) = get("source") {
                body.insert("source_details".into(), json!(format!("{} ({})", s, run.plan.source).chars().take(2000).collect::<String>()));
            }
            if let Some(o) = owner {
                body.insert("assigned_to".into(), json!(o));
            }
            match kind {
                "company" => {
                    let Some(name) = get("name") else { return Err(anyhow!("no company name")) };
                    body.insert("company_name".into(), json!(name));
                    if let Some(r) = get("annual_revenue") {
                        let cleaned: String = r.chars().filter(|c| c.is_ascii_digit() || *c == '.' || *c == '-').collect();
                        if !cleaned.is_empty() {
                            body.insert("annual_revenue".into(), json!(cleaned));
                        }
                    }
                }
                "contact" | "lead" => {
                    let (first, last) = name_parts(get("first_name").as_deref(), get("last_name").as_deref(), get("full_name").as_deref());
                    if let Some(f) = &first {
                        body.insert("first_name".into(), json!(f));
                    }
                    if let Some(l) = &last {
                        body.insert("last_name".into(), json!(l));
                    }
                    if let Some(j) = get("job_title") {
                        body.insert("job_title".into(), json!(j));
                    }
                    if kind == "contact" {
                        if first.is_none() && last.is_none() && multi.is_empty() {
                            return Err(anyhow!("no name, email or phone"));
                        }
                        if let Some(d) = get("birthdate").as_deref().and_then(parse_when) {
                            body.insert("birthdate".into(), json!(d.format("%Y-%m-%d").to_string()));
                        }
                        let company = match get("company_ref") {
                            Some(r) => run.find_ref(&["company"], &r).await?.map(|x| x.1),
                            None => None,
                        };
                        let company = match (company, get("company_name")) {
                            (Some(c), _) => Some(c),
                            (None, Some(n)) => Some(run.company_named(&n).await?),
                            _ => None,
                        };
                        if let Some(c) = company {
                            body.insert("company_id".into(), json!(c));
                        }
                    } else {
                        let company = get("company_name");
                        if let Some(c) = &company {
                            body.insert("company_name".into(), json!(c));
                        }
                        let who = [first.clone(), last.clone()].into_iter().flatten().collect::<Vec<_>>().join(" ");
                        let title = get("title").or(company.clone()).or_else(|| (!who.is_empty()).then_some(who)).ok_or_else(|| anyhow!("no title, name or company"))?;
                        body.insert("title".into(), json!(title.chars().take(300).collect::<String>()));
                        if let Some(s) = get("status")
                            && let Some(st) = run.stage("lead", "", &s).await? {
                                body.insert("stage_id".into(), json!(st));
                            }
                    }
                }
                "deal" => {
                    let title = get("title").ok_or_else(|| anyhow!("no deal name"))?;
                    body.insert("title".into(), json!(title.chars().take(300).collect::<String>()));
                    if let Some(a) = get("amount") {
                        let cleaned: String = a.chars().filter(|c| c.is_ascii_digit() || *c == '.' || *c == '-').collect();
                        if !cleaned.is_empty() && cleaned.parse::<f64>().is_ok() {
                            body.insert("opportunity".into(), json!(cleaned));
                            body.insert("is_manual_amount".into(), json!(true));
                        }
                    }
                    if let Some(c) = get("currency").map(|c| c.to_uppercase())
                        && run.currencies.contains(&c) {
                            body.insert("currency".into(), json!(c));
                        }
                    if let Some(d) = get("close_date").as_deref().and_then(parse_when) {
                        body.insert("close_date".into(), json!(d.format("%Y-%m-%d").to_string()));
                    }
                    if let Some(p) = get("probability").and_then(|p| p.trim_end_matches('%').trim().parse::<f64>().ok()) {
                        // Some systems export 0–1, others 0–100.
                        let p = if p <= 1.0 && p > 0.0 { p * 100.0 } else { p };
                        body.insert("probability".into(), json!(p.round().clamp(0.0, 100.0) as i64));
                    }
                    let pipeline = get("pipeline").unwrap_or_default();
                    if let Some(s) = get("stage") {
                        if let Some(st) = run.stage("deal", &pipeline, &s).await? {
                            let cat: Option<Uuid> = sqlx::query_scalar("SELECT category_id FROM crm_stages WHERE id = $1").bind(st).fetch_optional(&run.state.db).await?;
                            if let Some(c) = cat {
                                body.insert("category_id".into(), json!(c));
                            }
                            body.insert("stage_id".into(), json!(st));
                        }
                    } else if let Some(c) = run.category(&pipeline).await? {
                        body.insert("category_id".into(), json!(c));
                    }
                    let company = match get("company_ref") {
                        Some(r) => run.find_ref(&["company"], &r).await?.map(|x| x.1),
                        None => None,
                    };
                    let company = match (company, get("company_name")) {
                        (Some(c), _) => Some(c),
                        (None, Some(n)) => Some(run.company_named(&n).await?),
                        _ => None,
                    };
                    if let Some(c) = company {
                        body.insert("company_id".into(), json!(c));
                    }
                    let mut contacts: Vec<Uuid> = Vec::new();
                    for c in col(t, m, "contact_ref") {
                        if let Some(v) = row.get(c) {
                            for part in v.split([';', ',', '|']).map(str::trim).filter(|p| !p.is_empty()) {
                                if let Some(i) = run.known("contact", part).await?
                                    && !contacts.contains(&i) {
                                        contacts.push(i);
                                    }
                            }
                        }
                    }
                    if contacts.is_empty()
                        && let Some(n) = get("contact_name") {
                            let found: Option<Uuid> = sqlx::query_scalar(
                                "SELECT id FROM crm_items WHERE entity_type = 3 AND deleted_at IS NULL AND lower(trim(coalesce(first_name, '') || ' ' || coalesce(last_name, ''))) = lower(trim($1)) LIMIT 1",
                            )
                            .bind(&n)
                            .fetch_optional(&run.state.db)
                            .await?;
                            contacts.extend(found);
                        }
                    if !contacts.is_empty() {
                        body.insert("contact_ids".into(), json!(contacts));
                    }
                }
                _ => {}
            }
            if !comments.is_empty() {
                body.insert("comments".into(), json!(comments.join("\n\n").chars().take(20_000).collect::<String>()));
            }
            if !address.is_empty() && kind != "deal" {
                body.insert("address".into(), Value::Object(address));
            }

            // Brought over before, or already in NERP?
            let mut existing = match &source_id {
                Some(s) => run.known(kind, s).await?,
                None => None,
            };
            if existing.is_none() && matches!(kind, "contact" | "lead" | "company") {
                let (phones, emails): (Vec<_>, Vec<_>) = keys.iter().cloned().partition(|(k, _)| k == "phone");
                let phones: Vec<String> = phones.into_iter().map(|x| x.1).collect();
                let emails: Vec<String> = emails.into_iter().map(|x| x.1).collect();
                if !phones.is_empty() || !emails.is_empty() {
                    existing = sqlx::query_scalar(
                        "SELECT i.id FROM crm_multifields m JOIN crm_items i ON i.id = m.item_id
                         WHERE i.entity_type = $1 AND i.deleted_at IS NULL
                           AND ((m.kind = 'phone' AND m.normalized = ANY($2)) OR (m.kind = 'email' AND m.normalized = ANY($3))) LIMIT 1",
                    )
                    .bind(entity)
                    .bind(&phones)
                    .bind(&emails)
                    .fetch_optional(&run.state.db)
                    .await?;
                }
                if existing.is_none() && kind == "company"
                    && let Some(n) = get("name") {
                        existing = sqlx::query_scalar("SELECT id FROM crm_items WHERE entity_type = 4 AND deleted_at IS NULL AND lower(title) = lower($1) LIMIT 1")
                            .bind(n)
                            .fetch_optional(&run.state.db)
                            .await?;
                    }
            }
            match existing {
                Some(e) if run.plan.duplicates == "skip" => {
                    run.remember(kind, source_id.as_deref(), e, false).await?;
                    Ok(2)
                }
                Some(e) => {
                    // Keep the record's own contact details; add the export's notes below them.
                    body.remove("multifields");
                    body.remove("comments");
                    import_item(run.state, &run.auth, &et, Some(e), body, None, note).await.map_err(|e| anyhow!(e.to_string()))?;
                    run.remember(kind, source_id.as_deref(), e, false).await?;
                    Ok(1)
                }
                None => {
                    if !multi.is_empty() {
                        body.insert("multifields".into(), Value::Array(multi));
                    }
                    let id = import_item(run.state, &run.auth, &et, None, body, created_at, note).await.map_err(|e| anyhow!(e.to_string()))?;
                    run.remember(kind, source_id.as_deref(), id, true).await?;
                    Ok(0)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(name: &str, headers: &[&str]) -> Table {
        Table { name: name.into(), headers: headers.iter().map(|h| h.to_string()).collect(), rows: vec![] }
    }

    #[test]
    fn recognises_exports() {
        let hub = table("hubspot-crm-exports-all-contacts-2026-11-12.csv", &["Record ID", "First Name", "Last Name", "Email", "Phone Number", "Contact owner", "Associated Company IDs", "Company Name", "Create Date"]);
        assert_eq!(kind_of(&hub), "contact");
        let m = auto_mapping("contact", &hub.headers);
        assert_eq!(m["Record ID"], "id");
        assert_eq!(m["Contact owner"], "owner");
        assert_eq!(m["Associated Company IDs"], "company_ref");
        assert_eq!(m["Company Name"], "company_name");
        assert_eq!(m["Phone Number"], "phone");
        assert_eq!(source_of(&[hub]), "hubspot");

        let sf = table("Opportunity.csv", &["Id", "AccountId", "Name", "StageName", "Amount", "CloseDate", "OwnerId", "CurrencyIsoCode", "Probability"]);
        assert_eq!(kind_of(&sf), "deal");
        let m = auto_mapping("deal", &sf.headers);
        assert_eq!(m["Id"], "id");
        assert_eq!(m["AccountId"], "company_ref");
        assert_eq!(m["Name"], "title");
        assert_eq!(m["StageName"], "stage");
        assert_eq!(m["CurrencyIsoCode"], "currency");
        assert_eq!(source_of(&[sf]), "salesforce");

        let pd = table("deals-export.csv", &["Deal - ID", "Deal - Title", "Deal - Value", "Deal - Currency", "Deal - Pipeline", "Deal - Stage", "Deal - Organization", "Deal - Contact person", "Deal - Owner", "Deal - Expected close date"]);
        let m = auto_mapping("deal", &pd.headers);
        assert_eq!(m["Deal - ID"], "id");
        assert_eq!(m["Deal - Title"], "title");
        assert_eq!(m["Deal - Value"], "amount");
        assert_eq!(m["Deal - Pipeline"], "pipeline");
        assert_eq!(m["Deal - Organization"], "company_name");
        assert_eq!(m["Deal - Contact person"], "contact_name");
        assert_eq!(m["Deal - Expected close date"], "close_date");
        assert_eq!(source_of(&[pd]), "pipedrive");

        let zo = table("Accounts_001.csv", &["Record Id", "Account Name", "Phone", "Website", "Billing City", "Billing Country", "Account Owner"]);
        assert_eq!(kind_of(&zo), "company");
        let m = auto_mapping("company", &zo.headers);
        assert_eq!(m["Account Name"], "name");
        assert_eq!(m["Billing City"], "address.city");
        assert_eq!(m["Account Owner"], "owner");
        assert_eq!(source_of(&[zo]), "zoho");
    }

    #[test]
    fn values() {
        assert_eq!(semantics_of("Closed Won"), "success");
        assert_eq!(semantics_of("Closed Lost"), "failure");
        assert_eq!(semantics_of("Qualification"), "process");
        assert_eq!(name_parts(None, None, Some("Ada King Lovelace")), (Some("Ada King".into()), Some("Lovelace".into())));
        assert_eq!(parse_when("2026-03-04").unwrap().to_rfc3339(), "2026-03-04T00:00:00+00:00");
        assert_eq!(parse_when("03/04/2026 2:30 PM").unwrap().to_rfc3339(), "2026-03-04T14:30:00+00:00");
        assert!(parse_when("not a date").is_none());
    }
}
