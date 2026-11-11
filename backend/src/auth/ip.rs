//! Company rule: NERP may be used only from some IP address ranges (the office, the VPN).
//! Checked when signing in, when a session is renewed, and on every request (sessions, API
//! keys and download links). Administrators (who may change settings) can be left out of the
//! rule, so a wrong list cannot lock everyone out. Public pages (forms, shared links) are
//! not affected.

use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::Value;

use super::{permissions, AuthUser};
use crate::{
    error::{AppError, AppResult},
    state::AppState,
};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rules {
    pub ranges: Vec<(IpAddr, u8)>,
    pub admins_exempt: bool,
}

/// `10.0.0.0/8`, `192.168.1.7`, `2001:db8::/32`.
pub fn parse_range(s: &str) -> Option<(IpAddr, u8)> {
    let s = s.trim();
    let (addr, len) = match s.split_once('/') {
        Some((a, l)) => (a.trim(), Some(l.trim().parse::<u8>().ok()?)),
        None => (s, None),
    };
    let ip: IpAddr = addr.parse().ok()?;
    let max = if ip.is_ipv4() { 32 } else { 128 };
    let len = len.unwrap_or(max);
    (len <= max).then_some((ip.to_canonical(), len))
}

pub fn contains((net, len): (IpAddr, u8), ip: IpAddr) -> bool {
    match (net, ip.to_canonical()) {
        (IpAddr::V4(n), IpAddr::V4(i)) => {
            let mask = if len == 0 { 0 } else { u32::MAX << (32 - len) };
            (u32::from(n) & mask) == (u32::from(i) & mask)
        }
        (IpAddr::V6(n), IpAddr::V6(i)) => {
            let mask = if len == 0 { 0 } else { u128::MAX << (128 - len) };
            (u128::from(n) & mask) == (u128::from(i) & mask)
        }
        _ => false,
    }
}

impl Rules {
    pub fn from_settings(list: &Value, exempt: &Value) -> Rules {
        Rules {
            ranges: list.as_array().map(|a| a.iter().filter_map(|v| v.as_str().and_then(parse_range)).collect()).unwrap_or_default(),
            admins_exempt: exempt.as_bool().unwrap_or(true),
        }
    }

    /// Whether someone with these permissions may work from this address.
    pub fn allows(&self, ip: Option<&str>, can: impl Fn(&str) -> bool) -> bool {
        if self.ranges.is_empty() || (self.admins_exempt && can(permissions::SETTINGS_MANAGE)) {
            return true;
        }
        match ip.and_then(|i| i.parse::<IpAddr>().ok()) {
            Some(ip) => self.ranges.iter().any(|r| contains(*r, ip)),
            None => false,
        }
    }
}

static CACHE: Mutex<Option<(Instant, Rules)>> = Mutex::new(None);
const FRESH: Duration = Duration::from_secs(10);

/// The rules (read again at most every 10 seconds).
pub async fn rules(state: &AppState) -> AppResult<Rules> {
    if let Some((at, r)) = CACHE.lock().expect("ip rules lock").as_ref() {
        if at.elapsed() < FRESH {
            return Ok(r.clone());
        }
    }
    let rows: Vec<(String, Value)> = sqlx::query_as("SELECT key, value FROM settings WHERE key IN ('ip_allow', 'ip_allow_admins_exempt')").fetch_all(&state.db).await?;
    let get = |k: &str| rows.iter().find(|(key, _)| key == k).map(|(_, v)| v.clone()).unwrap_or(Value::Null);
    let r = Rules::from_settings(&get("ip_allow"), &get("ip_allow_admins_exempt"));
    *CACHE.lock().expect("ip rules lock") = Some((Instant::now(), r.clone()));
    Ok(r)
}

/// After the settings change here.
pub fn forget() {
    *CACHE.lock().expect("ip rules lock") = None;
}

fn refused(ip: Option<&str>) -> AppError {
    AppError::Blocked(format!("NERP can't be used from this network ({}); ask an administrator", ip.unwrap_or("unknown address")))
}

pub async fn check(state: &AppState, auth: &AuthUser, ip: Option<&str>) -> AppResult<()> {
    if rules(state).await?.allows(ip, |p| auth.can(p)) { Ok(()) } else { Err(refused(ip)) }
}

/// For sign-in, before there is a session.
pub async fn check_user(state: &AppState, user: uuid::Uuid, ip: Option<&str>) -> AppResult<()> {
    let r = rules(state).await?;
    if r.ranges.is_empty() {
        return Ok(());
    }
    let perms = super::load_permissions(state, user).await?;
    let can = |p: &str| perms.iter().any(|x| x == permissions::ALL || x == p);
    if r.allows(ip, can) { Ok(()) } else { Err(refused(ip)) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ranges() {
        let r = Rules::from_settings(&json!(["10.1.0.0/16", "203.0.113.7", "2001:db8::/32", "nonsense", "10.0.0.0/33"]), &json!(false));
        assert_eq!(r.ranges.len(), 3);
        let no = |_: &str| false;
        assert!(r.allows(Some("10.1.200.3"), no));
        assert!(!r.allows(Some("10.2.0.1"), no));
        assert!(r.allows(Some("203.0.113.7"), no) && !r.allows(Some("203.0.113.8"), no));
        assert!(r.allows(Some("::ffff:10.1.0.9"), no));
        assert!(r.allows(Some("2001:db8:1::5"), no) && !r.allows(Some("2001:db9::1"), no));
        assert!(!r.allows(None, no));
        assert!(!r.allows(Some("10.2.0.1"), |p| p == permissions::SETTINGS_MANAGE));
        let exempt = Rules { admins_exempt: true, ..r.clone() };
        assert!(exempt.allows(Some("10.2.0.1"), |p| p == permissions::SETTINGS_MANAGE));
        assert!(Rules::default().allows(None, no));
        assert!(contains(parse_range("0.0.0.0/0").unwrap(), "8.8.8.8".parse().unwrap()));
    }
}
