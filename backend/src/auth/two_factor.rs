//! Time-based one-time passwords (RFC 6238; works with any authenticator app) and
//! single-use recovery codes.

use totp_rs::{Builder, Secret, Totp};
use uuid::Uuid;

use super::tokens::hash_token;
use crate::{
    error::{AppError, AppResult},
    state::AppState,
};

pub const RECOVERY_CODE_COUNT: usize = 10;

pub fn new_secret() -> String {
    Secret::generate().to_base32()
}

pub fn build(secret_b32: &str, issuer: &str, account: &str) -> AppResult<Totp> {
    let secret = Secret::try_from_base32(secret_b32).map_err(|e| anyhow::anyhow!("bad TOTP secret: {e:?}"))?;
    // Issuer and account must not contain ':' in an otpauth URL.
    let issuer = issuer.replace(':', " ");
    let account = account.replace(':', " ");
    Builder::new()
        .with_secret(secret)
        .with_issuer(Some(issuer))
        .with_account_name(account)
        .build()
        .map_err(|e| AppError::Internal(anyhow::anyhow!("TOTP setup failed: {e:?}")))
}

/// Checks a 6-digit code. Each time step is accepted only once per user, so a code
/// that was shoulder-surfed or intercepted cannot be replayed.
pub async fn verify_code(state: &AppState, user: Uuid, secret_b32: &str, code: &str) -> AppResult<bool> {
    let code: String = code.chars().filter(|c| c.is_ascii_digit()).collect();
    if code.len() != 6 {
        return Ok(false);
    }
    let totp = build(secret_b32, "NERP", "user")?;
    let Some(step) = totp.check_current(&code) else {
        return Ok(false);
    };
    let mut redis = state.redis.clone();
    let fresh: bool = redis::cmd("SET")
        .arg(format!("auth:totp_used:{user}:{step}"))
        .arg(1)
        .arg("NX")
        .arg("EX")
        .arg(120)
        .query_async::<Option<String>>(&mut redis)
        .await?
        .is_some();
    Ok(fresh)
}

fn normalize_recovery(code: &str) -> String {
    code.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_lowercase()
}

/// Generates a fresh set of recovery codes, replacing any old ones. Returns them in clear
/// text once; only hashes are stored.
pub async fn regenerate_recovery_codes<'e, E>(executor: E, user: Uuid) -> AppResult<Vec<String>>
where
    E: sqlx::PgExecutor<'e> + Copy,
{
    const ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";
    let codes: Vec<String> = (0..RECOVERY_CODE_COUNT)
        .map(|_| {
            let mut bytes = [0u8; 10];
            rand::fill(&mut bytes);
            let s: String = bytes.iter().map(|b| ALPHABET[*b as usize % ALPHABET.len()] as char).collect();
            format!("{}-{}", &s[..5], &s[5..])
        })
        .collect();
    let hashes: Vec<String> = codes.iter().map(|c| hash_token(&normalize_recovery(c))).collect();
    sqlx::query!("DELETE FROM user_recovery_codes WHERE user_id = $1", user)
        .execute(executor)
        .await?;
    sqlx::query!("INSERT INTO user_recovery_codes (user_id, code_hash) SELECT $1, UNNEST($2::text[])", user, &hashes)
        .execute(executor)
        .await?;
    Ok(codes)
}

/// Uses up a recovery code if it is valid and unused.
pub async fn use_recovery_code(state: &AppState, user: Uuid, code: &str) -> AppResult<bool> {
    let normalized = normalize_recovery(code);
    if normalized.len() != 10 {
        return Ok(false);
    }
    let used = sqlx::query!("UPDATE user_recovery_codes SET used_at = now()
         WHERE user_id = $1 AND code_hash = $2 AND used_at IS NULL", user, hash_token(&normalized))
    .execute(&state.db)
    .await?
    .rows_affected();
    Ok(used == 1)
}

pub async fn remaining_recovery_codes(state: &AppState, user: Uuid) -> AppResult<i64> {
    Ok(sqlx::query_scalar!(r#"SELECT COUNT(*) AS "v!" FROM user_recovery_codes WHERE user_id = $1 AND used_at IS NULL"#, user)
    .fetch_one(&state.db)
    .await?)
}
