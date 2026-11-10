use argon2::{
    password_hash::{phc::PasswordHash, PasswordHasher, PasswordVerifier},
    Argon2,
};

use crate::error::{AppError, AppResult};

pub const MIN_PASSWORD_LEN: usize = 8;

pub fn validate_password(password: &str) -> AppResult<()> {
    if password.chars().count() < MIN_PASSWORD_LEN {
        return Err(AppError::bad(format!(
            "password must be at least {MIN_PASSWORD_LEN} characters"
        )));
    }
    if password.len() > 256 {
        return Err(AppError::bad("password is too long"));
    }
    Ok(())
}

/// Hashing is CPU-heavy, so it runs on the blocking thread pool.
pub async fn hash_password(password: String) -> AppResult<String> {
    tokio::task::spawn_blocking(move || {
        PasswordHasher::<PasswordHash>::hash_password(&Argon2::default(), password.as_bytes())
            .map(|h| h.to_string())
            .map_err(|e| anyhow::anyhow!("hash error: {e}"))
    })
    .await
    .map_err(|e| anyhow::anyhow!(e))?
    .map_err(AppError::from)
}

pub async fn verify_password(password: String, hash: String) -> AppResult<bool> {
    tokio::task::spawn_blocking(move || {
        PasswordVerifier::<str>::verify_password(&Argon2::default(), password.as_bytes(), hash.as_str()).is_ok()
    })
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))
}

/// A valid hash of a random password, used to keep login timing uniform for unknown emails.
pub fn dummy_hash() -> &'static str {
    static HASH: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    HASH.get_or_init(|| {
        PasswordHasher::<PasswordHash>::hash_password(&Argon2::default(), b"nerp-timing-equaliser")
            .expect("hash")
            .to_string()
    })
}
