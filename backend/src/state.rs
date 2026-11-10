use crate::{config::Config, mail::Mailer, realtime::Hub};
use jsonwebtoken::{DecodingKey, EncodingKey};
use redis::aio::ConnectionManager;
use sqlx::PgPool;
use std::sync::Arc;
use tokio::sync::Notify;

#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    pub redis: ConnectionManager,
    pub config: Arc<Config>,
    pub jwt: Arc<JwtKeys>,
    pub mailer: Arc<Mailer>,
    /// Local WebSocket connections.
    pub hub: Arc<Hub>,
    /// Wakes this instance's job worker as soon as a job is queued.
    pub job_wakeup: Arc<Notify>,
    /// Identifies this process in job leases.
    pub instance_id: String,
}

pub struct JwtKeys {
    pub encoding: EncodingKey,
    pub decoding: DecodingKey,
}

impl JwtKeys {
    pub fn new(secret: &str) -> Self {
        Self {
            encoding: EncodingKey::from_secret(secret.as_bytes()),
            decoding: DecodingKey::from_secret(secret.as_bytes()),
        }
    }
}
