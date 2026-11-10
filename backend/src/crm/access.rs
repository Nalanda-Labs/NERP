//! CRM access rights.
//!
//! Each role grants, per record type and action, a scope:
//!   none < own < department < subdepartments < all
//! "own" means records the user is responsible for; "department" adds records owned by
//! anyone in the user's departments; "subdepartments" also includes their sub-departments.
//! A user's effective scope is the widest one across their roles. Administrators (`*`)
//! and holders of `crm.admin` see and change everything.

use serde::Serialize;
use sqlx::{Postgres, QueryBuilder};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

use crate::{
    auth::{permissions as perm, AuthUser},
    error::{AppError, AppResult},
    state::AppState,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    None,
    Own,
    Department,
    Subdepartments,
    All,
}

impl Scope {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "none" => Self::None,
            "own" => Self::Own,
            "department" => Self::Department,
            "subdepartments" => Self::Subdepartments,
            "all" => Self::All,
            _ => return None,
        })
    }
}

pub const ACTIONS: &[&str] = &["read", "add", "write", "delete", "export", "import"];

pub fn is_crm_admin(auth: &AuthUser) -> bool {
    auth.can(perm::CRM_ADMIN)
}

/// The caller's scope for one action on one record type.
pub async fn scope(state: &AppState, auth: &AuthUser, entity_type: i16, action: &str) -> AppResult<Scope> {
    if is_crm_admin(auth) {
        return Ok(Scope::All);
    }
    let scopes: Vec<String> = sqlx::query_scalar(
        "SELECT p.scope FROM crm_role_permissions p JOIN user_roles ur ON ur.role_id = p.role_id
         WHERE ur.user_id = $1 AND p.entity_type = $2 AND p.action = $3",
    )
    .bind(auth.id)
    .bind(entity_type)
    .bind(action)
    .fetch_all(&state.db)
    .await?;
    Ok(scopes.iter().filter_map(|s| Scope::parse(s)).max().unwrap_or(Scope::None))
}

/// Users whose records fall inside a department-based scope (including the caller).
pub async fn team(state: &AppState, user: Uuid, scope: Scope) -> AppResult<Vec<Uuid>> {
    let mine: Vec<Uuid> = sqlx::query_scalar("SELECT department_id FROM user_departments WHERE user_id = $1")
        .bind(user)
        .fetch_all(&state.db)
        .await?;
    let mut depts: HashSet<Uuid> = mine.iter().copied().collect();
    if scope == Scope::Subdepartments && !mine.is_empty() {
        let edges: Vec<(Uuid, Option<Uuid>)> = sqlx::query_as("SELECT id, parent_id FROM departments")
            .fetch_all(&state.db)
            .await?;
        let mut children: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
        for (id, parent) in edges {
            if let Some(p) = parent {
                children.entry(p).or_default().push(id);
            }
        }
        let mut stack: Vec<Uuid> = mine.clone();
        while let Some(d) = stack.pop() {
            for c in children.get(&d).into_iter().flatten() {
                if depts.insert(*c) {
                    stack.push(*c);
                }
            }
        }
    }
    let ids: Vec<Uuid> = depts.into_iter().collect();
    let mut users: Vec<Uuid> = sqlx::query_scalar("SELECT DISTINCT user_id FROM user_departments WHERE department_id = ANY($1)")
        .bind(&ids)
        .fetch_all(&state.db)
        .await?;
    if !users.contains(&user) {
        users.push(user);
    }
    Ok(users)
}

/// A resolved scope, ready to be turned into SQL or checked against a record.
pub enum Visible {
    Nothing,
    /// Records of these responsible people, plus (for reading) those the given employee
    /// observes.
    Owners(Vec<Uuid>, Option<Uuid>),
    Everything,
}

pub async fn visible(state: &AppState, auth: &AuthUser, entity_type: i16, action: &str) -> AppResult<Visible> {
    let observer = (action == "read").then_some(auth.id);
    Ok(match scope(state, auth, entity_type, action).await? {
        Scope::None => Visible::Nothing,
        Scope::Own => Visible::Owners(vec![auth.id], observer),
        s @ (Scope::Department | Scope::Subdepartments) => Visible::Owners(team(state, auth.id, s).await?, observer),
        Scope::All => Visible::Everything,
    })
}

impl Visible {
    /// Appends `AND <record i is within scope>`.
    pub fn push(&self, qb: &mut QueryBuilder<Postgres>) {
        match self {
            Visible::Nothing => {
                qb.push(" AND FALSE");
            }
            Visible::Owners(users, None) => {
                qb.push(" AND i.assigned_to = ANY(").push_bind(users.clone()).push(")");
            }
            Visible::Owners(users, Some(me)) => {
                qb.push(" AND (i.assigned_to = ANY(").push_bind(users.clone())
                    .push(") OR EXISTS (SELECT 1 FROM crm_item_observers o WHERE o.item_id = i.id AND o.user_id = ").push_bind(*me).push("))");
            }
            Visible::Everything => {}
        }
    }

    pub fn allows(&self, assigned_to: Option<Uuid>) -> bool {
        match self {
            Visible::Nothing => false,
            Visible::Owners(users, _) => assigned_to.is_some_and(|a| users.contains(&a)),
            Visible::Everything => true,
        }
    }
}

/// Fails unless the caller may perform `action` on a record owned by `assigned_to`.
pub async fn require(state: &AppState, auth: &AuthUser, entity_type: i16, action: &str, assigned_to: Option<Uuid>) -> AppResult<()> {
    if visible(state, auth, entity_type, action).await?.allows(assigned_to) {
        Ok(())
    } else if action == "read" {
        Err(AppError::NotFound("record"))
    } else {
        Err(AppError::Forbidden)
    }
}

pub async fn is_observer(state: &AppState, item: Uuid, user: Uuid) -> AppResult<bool> {
    Ok(sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM crm_item_observers WHERE item_id = $1 AND user_id = $2)")
        .bind(item)
        .bind(user)
        .fetch_one(&state.db)
        .await?)
}

/// Read access to one record: by the caller's scope, or as one of its observers.
pub async fn require_read(state: &AppState, auth: &AuthUser, entity_type: i16, item: Uuid, assigned_to: Option<Uuid>) -> AppResult<()> {
    if visible(state, auth, entity_type, "read").await?.allows(assigned_to) || is_observer(state, item, auth.id).await? {
        Ok(())
    } else {
        Err(AppError::NotFound("record"))
    }
}

/// Fails unless the caller may perform `action` on at least some records of this type.
pub async fn require_any(state: &AppState, auth: &AuthUser, entity_type: i16, action: &str) -> AppResult<Scope> {
    let s = scope(state, auth, entity_type, action).await?;
    if s == Scope::None { Err(AppError::Forbidden) } else { Ok(s) }
}

#[derive(Serialize)]
pub struct Rights {
    pub read: Scope,
    pub add: Scope,
    pub write: Scope,
    pub delete: Scope,
    pub export: Scope,
    pub import: Scope,
}

pub async fn rights(state: &AppState, auth: &AuthUser, entity_type: i16) -> AppResult<Rights> {
    Ok(Rights {
        read: scope(state, auth, entity_type, "read").await?,
        add: scope(state, auth, entity_type, "add").await?,
        write: scope(state, auth, entity_type, "write").await?,
        delete: scope(state, auth, entity_type, "delete").await?,
        export: scope(state, auth, entity_type, "export").await?,
        import: scope(state, auth, entity_type, "import").await?,
    })
}
