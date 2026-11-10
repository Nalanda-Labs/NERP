//! LDAP: syncing employees from the directory and checking their passwords against it.

use std::time::Duration;

use axum::{extract::State, Json};
use ldap3::{
    adapters::{Adapter, EntriesOnly, PagedResults},
    ldap_escape, Ldap, LdapConnAsync, LdapConnSettings, Scope, SearchEntry,
};
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use super::{bad, load, SecretUpdate};
use crate::{
    audit,
    auth::{invalidate_authz, permissions as perm, tokens, AuthUser},
    error::{AppError, AppResult},
    routes::normalize_email,
    state::AppState,
};

pub const SYNC_JOB: &str = "directory.sync";
const TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct LdapConfig {
    pub enabled: bool,
    /// ldap://host:389 or ldaps://host:636
    pub url: String,
    /// Upgrade an ldap:// connection with StartTLS.
    pub starttls: bool,
    /// Accept any certificate (self-signed test directories only).
    pub skip_tls_verify: bool,
    /// The service account used to search the directory (empty = anonymous).
    pub bind_dn: String,
    pub base_dn: String,
    /// Which entries are employees.
    pub user_filter: String,
    pub attr_id: String,
    pub attr_email: String,
    pub attr_first_name: String,
    pub attr_last_name: String,
    pub attr_phone: String,
    pub attr_position: String,
    /// Matched by name to a NERP department when the account is created.
    pub attr_department: String,
    /// Sync every hour.
    pub auto_sync: bool,
}

impl Default for LdapConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            url: String::new(),
            starttls: false,
            skip_tls_verify: false,
            bind_dn: String::new(),
            base_dn: String::new(),
            user_filter: "(objectClass=person)".into(),
            attr_id: "uid".into(),
            attr_email: "mail".into(),
            attr_first_name: "givenName".into(),
            attr_last_name: "sn".into(),
            attr_phone: "telephoneNumber".into(),
            attr_position: "title".into(),
            attr_department: "department".into(),
            auto_sync: true,
        }
    }
}

impl LdapConfig {
    fn attrs(&self) -> Vec<String> {
        let mut v: Vec<String> = [
            &self.attr_id,
            &self.attr_email,
            &self.attr_first_name,
            &self.attr_last_name,
            &self.attr_phone,
            &self.attr_position,
            &self.attr_department,
        ]
        .iter()
        .filter(|a| !a.is_empty())
        .map(|a| a.to_string())
        .collect();
        v.push("cn".into());
        v.sort();
        v.dedup();
        v
    }

    fn validate(&mut self) -> AppResult<()> {
        for s in [
            &mut self.url,
            &mut self.bind_dn,
            &mut self.base_dn,
            &mut self.user_filter,
            &mut self.attr_id,
            &mut self.attr_email,
            &mut self.attr_first_name,
            &mut self.attr_last_name,
            &mut self.attr_phone,
            &mut self.attr_position,
            &mut self.attr_department,
        ] {
            *s = s.trim().to_string();
            if s.len() > 500 {
                return Err(bad("a directory setting is too long"));
            }
        }
        if !self.url.is_empty() && !(self.url.starts_with("ldap://") || self.url.starts_with("ldaps://")) {
            return Err(bad("the address must start with ldap:// or ldaps://"));
        }
        if self.user_filter.is_empty() {
            self.user_filter = LdapConfig::default().user_filter;
        }
        if !(self.user_filter.starts_with('(') && self.user_filter.ends_with(')')) {
            return Err(bad("the filter must be in parentheses, e.g. (objectClass=person)"));
        }
        let attr_ok = |a: &str| a.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == ';');
        for a in [&self.attr_id, &self.attr_email, &self.attr_first_name, &self.attr_last_name, &self.attr_phone, &self.attr_position, &self.attr_department] {
            if !attr_ok(a) {
                return Err(bad(format!("{a:?} is not an attribute name")));
            }
        }
        if self.attr_email.is_empty() {
            return Err(bad("the email attribute is required"));
        }
        if self.enabled && (self.url.is_empty() || self.base_dn.is_empty()) {
            return Err(bad("set the directory address and base DN before turning it on"));
        }
        Ok(())
    }
}

/// An employee as the directory describes them.
#[derive(Debug, Clone, Serialize)]
pub struct Person {
    pub dn: String,
    pub id: String,
    pub email: String,
    pub first_name: String,
    pub last_name: String,
    pub phone: Option<String>,
    pub position: Option<String>,
    pub department: Option<String>,
}

fn person(cfg: &LdapConfig, e: SearchEntry) -> Option<Person> {
    let get = |a: &str| -> Option<String> {
        if a.is_empty() {
            return None;
        }
        e.attrs
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(a))
            .and_then(|(_, v)| v.iter().map(|s| s.trim()).find(|s| !s.is_empty()))
            .map(|s| s.chars().take(200).collect())
    };
    let email = normalize_email(&get(&cfg.attr_email)?).ok()?;
    // Binary ids (Active Directory's objectGUID) arrive as bytes.
    let id = get(&cfg.attr_id)
        .or_else(|| {
            e.bin_attrs
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(&cfg.attr_id))
                .and_then(|(_, v)| v.first())
                .map(|b| b.iter().map(|x| format!("{x:02x}")).collect())
        })
        .unwrap_or_else(|| e.dn.to_lowercase());
    let cn = get("cn");
    let first_name = get(&cfg.attr_first_name)
        .or_else(|| cn.as_ref().and_then(|c| c.split_whitespace().next().map(String::from)))
        .unwrap_or_else(|| email.split('@').next().unwrap_or_default().to_string());
    let last_name = get(&cfg.attr_last_name)
        .or_else(|| cn.as_ref().map(|c| c.split_whitespace().skip(1).collect::<Vec<_>>().join(" ")).filter(|s| !s.is_empty()))
        .unwrap_or_else(|| "-".into());
    Some(Person {
        dn: e.dn.clone(),
        id,
        email,
        first_name,
        last_name,
        phone: get(&cfg.attr_phone),
        position: get(&cfg.attr_position),
        department: get(&cfg.attr_department),
    })
}

async fn connect(cfg: &LdapConfig) -> Result<Ldap, String> {
    let settings = LdapConnSettings::new()
        .set_conn_timeout(Duration::from_secs(10))
        .set_starttls(cfg.starttls)
        .set_no_tls_verify(cfg.skip_tls_verify);
    let (conn, ldap) = LdapConnAsync::with_settings(settings, &cfg.url).await.map_err(|e| format!("cannot reach the directory: {e}"))?;
    tokio::spawn(async move {
        if let Err(e) = conn.drive().await {
            tracing::debug!(error = %e, "ldap connection closed");
        }
    });
    Ok(ldap)
}

/// Connected as the service account.
async fn service(cfg: &LdapConfig, secret: Option<&str>) -> Result<Ldap, String> {
    let mut ldap = connect(cfg).await?;
    if !cfg.bind_dn.is_empty() {
        let res = tokio::time::timeout(TIMEOUT, ldap.simple_bind(&cfg.bind_dn, secret.unwrap_or_default()))
            .await
            .map_err(|_| "the directory did not answer".to_string())?
            .map_err(|e| format!("cannot reach the directory: {e}"))?;
        if res.rc != 0 {
            return Err(format!("the directory refused the service account ({})", res.text.trim()).replace(" ()", ""));
        }
    }
    Ok(ldap)
}

async fn search(ldap: &mut Ldap, cfg: &LdapConfig, filter: &str) -> Result<Vec<Person>, String> {
    let work = async {
        let adapters: Vec<Box<dyn Adapter<'_, String, Vec<String>>>> = vec![Box::new(EntriesOnly::new()), Box::new(PagedResults::new(500))];
        let mut stream = ldap.streaming_search_with(adapters, &cfg.base_dn, Scope::Subtree, filter, cfg.attrs()).await.map_err(|e| format!("search failed: {e}"))?;
        let mut out = Vec::new();
        while let Some(entry) = stream.next().await.map_err(|e| format!("search failed: {e}"))? {
            if let Some(p) = person(cfg, SearchEntry::construct(entry)) {
                out.push(p);
            }
        }
        let res = stream.finish().await;
        if res.rc != 0 {
            return Err(format!("search failed: {} {}", res.rc, res.text.trim()));
        }
        Ok(out)
    };
    tokio::time::timeout(Duration::from_secs(120), work).await.map_err(|_| "the directory did not answer".to_string())?
}

/// Checks a password against the directory. Ok(None): no such person, or a wrong password.
pub async fn authenticate(cfg: &LdapConfig, secret: Option<&str>, email: &str, password: &str) -> Result<Option<Person>, String> {
    // An empty password would be an "unauthenticated bind", which servers accept.
    if password.is_empty() {
        return Ok(None);
    }
    let mut ldap = service(cfg, secret).await?;
    let filter = format!("(&{}({}={}))", cfg.user_filter, cfg.attr_email, ldap_escape(email));
    let found = search(&mut ldap, cfg, &filter).await?;
    let _ = ldap.unbind().await;
    let p = match found.as_slice() {
        [] => return Ok(None),
        [p] => p.clone(),
        _ => return Err(format!("several directory entries have the email {email}")),
    };
    let mut user = connect(cfg).await?;
    let res = tokio::time::timeout(TIMEOUT, user.simple_bind(&p.dn, password))
        .await
        .map_err(|_| "the directory did not answer".to_string())?
        .map_err(|e| format!("cannot reach the directory: {e}"))?;
    let _ = user.unbind().await;
    match res.rc {
        0 => Ok(Some(p)),
        // invalidCredentials, unwillingToPerform (disabled), inappropriateAuthentication
        49 | 53 | 48 => Ok(None),
        rc => Err(format!("the directory answered {rc} {}", res.text.trim())),
    }
}

#[derive(Debug, PartialEq)]
enum Outcome {
    Created,
    Updated,
    Unchanged,
}

/// Creates or updates the NERP account for a directory entry. An existing account with the
/// same email is linked to the entry (and from then on signs in with the directory password).
async fn upsert(state: &AppState, p: &Person) -> AppResult<(Uuid, Outcome, bool)> {
    match upsert_once(state, p).await {
        // Someone else made the account at the same moment (two first sign-ins at once): use it.
        Err(AppError::Database(e)) if crate::error::is_unique_violation(&e) => upsert_once(state, p).await,
        other => other,
    }
}

async fn upsert_once(state: &AppState, p: &Person) -> AppResult<(Uuid, Outcome, bool)> {
    type Row = (Uuid, String, String, String, Option<String>, Option<String>, Option<String>, bool, bool, String, Option<String>);
    let sel = "SELECT id, email, first_name, last_name, phone, position, ldap_dn, is_active, directory_deactivated, auth_source, external_id FROM users";
    let mut row: Option<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!("{sel} WHERE auth_source = 'ldap' AND external_id = $1")))
        .bind(&p.id)
        .fetch_optional(&state.db)
        .await?;
    if row.is_none() {
        row = sqlx::query_as(sqlx::AssertSqlSafe(format!("{sel} WHERE email = $1"))).bind(&p.email).fetch_optional(&state.db).await?;
    }
    if let Some((id, email, first, last, phone, position, dn, active, dir_off, source, ext)) = row {
        let reactivate = !active && dir_off;
        let changed = email != p.email
            || first != p.first_name
            || last != p.last_name
            || (p.phone.is_some() && phone != p.phone)
            || (p.position.is_some() && position != p.position)
            || dn.as_deref() != Some(p.dn.as_str())
            || source != "ldap"
            || ext.as_deref() != Some(p.id.as_str())
            || reactivate;
        if !changed {
            return Ok((id, Outcome::Unchanged, false));
        }
        let update = |with_email: bool| {
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "UPDATE users SET {} first_name = $2, last_name = $3, phone = COALESCE($4, phone), position = COALESCE($5, position),
                    ldap_dn = $6, auth_source = 'ldap', external_id = $7, updated_at = now(),
                    is_active = is_active OR directory_deactivated, directory_deactivated = FALSE
                 WHERE id = $1",
                if with_email { "email = $8," } else { "" }
            )))
            .bind(id)
            .bind(&p.first_name)
            .bind(&p.last_name)
            .bind(&p.phone)
            .bind(&p.position)
            .bind(&p.dn)
            .bind(&p.id)
            .bind(&p.email)
        };
        if let Err(e) = update(true).execute(&state.db).await {
            // The new email belongs to someone else: keep the old one.
            if !crate::error::is_unique_violation(&e) {
                return Err(e.into());
            }
            update(false).execute(&state.db).await?;
        }
        if reactivate {
            invalidate_authz(state, &[id]).await?;
        }
        return Ok((id, Outcome::Updated, reactivate));
    }

    let mut tx = state.db.begin().await?;
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO users (email, password_hash, first_name, last_name, phone, position, auth_source, external_id, ldap_dn)
         VALUES ($1, '!ldap', $2, $3, $4, $5, 'ldap', $6, $7) RETURNING id",
    )
    .bind(&p.email)
    .bind(&p.first_name)
    .bind(&p.last_name)
    .bind(&p.phone)
    .bind(&p.position)
    .bind(&p.id)
    .bind(&p.dn)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO user_roles (user_id, role_id) SELECT $1, id FROM roles WHERE code = 'employee'").bind(id).execute(&mut *tx).await?;
    if let Some(d) = &p.department {
        sqlx::query(
            "INSERT INTO user_departments (user_id, department_id)
             SELECT $1, id FROM departments WHERE lower(name) = lower($2) ORDER BY id LIMIT 1",
        )
        .bind(id)
        .bind(d)
        .execute(&mut *tx)
        .await?;
    }
    audit::record(&mut *tx, None, "user.create", "user", Some(id.to_string()), json!({ "email": p.email, "source": "ldap" }), None).await;
    tx.commit().await?;
    Ok((id, Outcome::Created, false))
}

/// Login: Ok(Some(user)) when the directory accepts the password (the account is created
/// on first sign-in); Ok(None) for a wrong password or someone not in the directory.
pub async fn sign_in(state: &AppState, email: &str, password: &str) -> AppResult<Option<Uuid>> {
    if !state.config.ldap_enabled {
        return Ok(None);
    }
    let s = load(state).await?;
    if !s.ldap.enabled {
        return Ok(None);
    }
    match authenticate(&s.ldap, s.ldap_secret.as_deref(), email, password).await {
        Ok(Some(p)) => Ok(Some(upsert(state, &p).await?.0)),
        Ok(None) => Ok(None),
        // Not the directory's fault, and the person can do nothing about it but ask.
        Err(e) if e.starts_with("several directory entries") => {
            tracing::warn!(error = %e, "directory sign-in refused");
            Err(AppError::BadRequest(format!("{e}; ask your administrator to fix the directory")))
        }
        Err(e) => {
            tracing::warn!(error = %e, "directory sign-in failed");
            Err(AppError::BadRequest("the company directory cannot be reached; try again in a few minutes".into()))
        }
    }
}

pub async fn enabled(state: &AppState) -> AppResult<bool> {
    if !state.config.ldap_enabled {
        return Ok(false);
    }
    let on: Option<bool> = sqlx::query_scalar("SELECT COALESCE((ldap ->> 'enabled')::boolean, FALSE) FROM directory_config WHERE id = 1")
        .fetch_optional(&state.db)
        .await?;
    Ok(on.unwrap_or(false))
}

/// Brings accounts in line with the directory: creates newcomers, updates details and
/// deactivates people who left.
pub async fn sync(state: &AppState, actor: Option<Uuid>) -> AppResult<Value> {
    available(state)?;
    let s = load(state).await?;
    if !s.ldap.enabled {
        return Err(bad("turn the directory on first"));
    }
    let mut redis = state.redis.clone();
    let got: Option<String> = redis::cmd("SET").arg("directory:sync").arg(&state.instance_id).arg("NX").arg("EX").arg(600).query_async(&mut redis).await?;
    if got.is_none() {
        return Err(AppError::Conflict("a sync is already running".into()));
    }
    let result = sync_locked(state, &s.ldap, s.ldap_secret.as_deref()).await;
    let _: () = redis.del("directory:sync").await?;
    let result = match result {
        Ok(v) => v,
        Err(AppError::BadRequest(e)) => json!({ "error": e }),
        Err(e) => return Err(e),
    };
    sqlx::query("UPDATE directory_config SET ldap_synced_at = now(), ldap_sync_result = $1 WHERE id = 1").bind(&result).execute(&state.db).await?;
    audit::record(&state.db, actor, "directory.sync", "settings", None, result.clone(), None).await;
    Ok(result)
}

async fn sync_locked(state: &AppState, cfg: &LdapConfig, secret: Option<&str>) -> AppResult<Value> {
    let people = async {
        let mut ldap = service(cfg, secret).await?;
        let r = search(&mut ldap, cfg, &cfg.user_filter).await;
        let _ = ldap.unbind().await;
        r
    }
    .await
    .map_err(AppError::BadRequest)?;
    // A filter or base that suddenly matches nobody must not deactivate everyone.
    if people.is_empty() {
        return Err(bad("the directory returned no employees, so nothing was changed; check the base DN and filter"));
    }
    let (mut created, mut updated, mut reactivated, mut skipped) = (0, 0, 0, 0);
    // An email (or id) on several entries is ambiguous: none of them is used.
    let mut count = std::collections::HashMap::<String, usize>::new();
    for p in &people {
        *count.entry(format!("e:{}", p.email)).or_default() += 1;
        *count.entry(format!("i:{}", p.id)).or_default() += 1;
    }
    let mut ids = Vec::new();
    for p in &people {
        if count[&format!("e:{}", p.email)] > 1 || count[&format!("i:{}", p.id)] > 1 {
            tracing::warn!(email = %p.email, "directory sync skipped an entry whose email or id is on several entries");
            skipped += 1;
            // Still in the directory: not deactivated for being ambiguous.
            ids.push(p.id.clone());
            continue;
        }
        match upsert(state, p).await {
            Ok((_, o, re)) => {
                ids.push(p.id.clone());
                match o {
                    Outcome::Created => created += 1,
                    Outcome::Updated => updated += 1,
                    Outcome::Unchanged => {}
                }
                if re {
                    reactivated += 1;
                }
            }
            Err(e) => {
                tracing::warn!(email = %p.email, error = %e, "directory sync skipped an entry");
                skipped += 1;
            }
        }
    }
    let gone: Vec<Uuid> = sqlx::query_scalar(
        "UPDATE users SET is_active = FALSE, directory_deactivated = TRUE, updated_at = now()
         WHERE auth_source = 'ldap' AND is_active AND NOT (external_id = ANY($1)) RETURNING id",
    )
    .bind(&ids)
    .fetch_all(&state.db)
    .await?;
    if !gone.is_empty() {
        invalidate_authz(state, &gone).await?;
        for id in &gone {
            tokens::revoke_all_sessions(state, *id, None).await?;
        }
    }
    Ok(json!({
        "found": people.len(),
        "created": created,
        "updated": updated,
        "reactivated": reactivated,
        "deactivated": gone.len(),
        "skipped": skipped,
    }))
}

pub async fn run_sync(state: &AppState) -> anyhow::Result<()> {
    if !state.config.ldap_enabled {
        return Ok(());
    }
    let s = load(state).await?;
    if s.ldap.enabled && s.ldap.auto_sync {
        match sync(state, None).await {
            Ok(r) => tracing::info!(result = %r, "directory synced"),
            Err(AppError::Conflict(_)) => {}
            Err(e) => return Err(anyhow::anyhow!("{e}")),
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

/// LDAP is offered only when the server is started with LDAP_ENABLED=true.
fn available(state: &AppState) -> AppResult<()> {
    if state.config.ldap_enabled {
        Ok(())
    } else {
        Err(bad("LDAP is turned off on this server (LDAP_ENABLED)"))
    }
}

#[derive(Deserialize)]
pub struct LdapUpdate {
    #[serde(flatten)]
    cfg: LdapConfig,
    #[serde(default)]
    bind_password: SecretUpdate,
}

pub async fn put_config(State(state): State<AppState>, auth: AuthUser, Json(mut b): Json<LdapUpdate>) -> AppResult<Json<Value>> {
    auth.require(perm::SETTINGS_MANAGE)?;
    available(&state)?;
    b.cfg.validate()?;
    let secret = b.bind_password.sealed(&state);
    sqlx::query("UPDATE directory_config SET ldap = $1, ldap_secret = CASE WHEN $2 THEN $3 ELSE ldap_secret END WHERE id = 1")
        .bind(serde_json::to_value(&b.cfg).map_err(anyhow::Error::from)?)
        .bind(secret.is_some())
        .bind(secret.flatten())
        .execute(&state.db)
        .await?;
    audit::record(&state.db, Some(auth.id), "directory.ldap_update", "settings", None, json!({ "enabled": b.cfg.enabled, "url": b.cfg.url }), None).await;
    super::get_config(State(state), auth).await
}

/// Connects with the saved settings and lists a few of the employees it finds.
pub async fn test(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    auth.require(perm::SETTINGS_MANAGE)?;
    available(&state)?;
    let s = load(&state).await?;
    if s.ldap.url.is_empty() || s.ldap.base_dn.is_empty() {
        return Err(bad("set the directory address and base DN first"));
    }
    let r = async {
        let mut ldap = service(&s.ldap, s.ldap_secret.as_deref()).await?;
        let r = search(&mut ldap, &s.ldap, &s.ldap.user_filter).await;
        let _ = ldap.unbind().await;
        r
    }
    .await;
    Ok(Json(match r {
        Ok(people) => json!({
            "ok": true,
            "found": people.len(),
            "sample": people.iter().take(5).map(|p| json!({ "email": p.email, "name": format!("{} {}", p.first_name, p.last_name), "dn": p.dn })).collect::<Vec<_>>(),
        }),
        Err(e) => json!({ "ok": false, "error": e }),
    }))
}

pub async fn sync_now(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    auth.require(perm::SETTINGS_MANAGE)?;
    let r = sync(&state, Some(auth.id)).await?;
    Ok(Json(r))
}
