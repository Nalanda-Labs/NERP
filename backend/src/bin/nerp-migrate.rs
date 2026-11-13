//! Applies NERP's database migrations and exits.
//!
//! The server's SQL is checked against the database schema at compile time (sqlx's `query!`
//! macros), so the database named by `DATABASE_URL` must be migrated before the server is built:
//!
//! ```sh
//! cargo run --bin nerp-migrate && cargo build --bin nerp
//! ```
//!
//! This binary contains no checked queries itself, so it builds without a database. The server
//! still applies any pending migrations when it starts, so deploying a new binary to an older
//! database works as before.

#[path = "../migrate.rs"]
mod migrate;

use anyhow::{Context, Result};
use sqlx::postgres::PgPoolOptions;

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,sqlx=warn".into()))
        .init();
    let url = std::env::var("DATABASE_URL").context("DATABASE_URL is not set")?;
    migrate::ensure_database(&url).await?;
    let db = PgPoolOptions::new().max_connections(1).connect(&url).await.context("connecting to PostgreSQL")?;
    migrate::run(&db).await.context("running migrations")?;
    tracing::info!("database migrations are up to date");
    Ok(())
}
