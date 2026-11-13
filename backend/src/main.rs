mod api;
mod audit;
mod audit_chain;
mod export;
mod health;
mod privacy;
mod retention;
mod sms;
mod commissions;
mod portal;
mod field;
mod sandbox;
mod teams;
mod attachments;
mod auth;
mod bi;
mod blob;
mod boards;
mod booking;
mod config;
mod crm;
mod crypto;
mod error;
mod jobs;
mod migrate;
mod mail;
mod meetings;
mod marketing;
mod shop;
mod inventory;
mod chat;
mod calendar;
mod im;
mod feed;
mod guests;
mod kb;
mod kb_import;
mod kb_collab;
mod hr;
mod site_tools;
mod solutions;
mod sites;
mod accounting;
mod calls;
mod lists;
mod workflows;
mod workflows_crm;
mod directory;
mod drive;
mod livekit;
mod notify;
mod office;
mod outbound;
mod realtime;
mod routes;
mod security_headers;
mod state;
mod support;
mod tasks;
mod training;
mod tracking;
mod video;
mod mcp;

use anyhow::{Context, Result};
use axum::{
    http::{header, Extensions, HeaderMap, HeaderValue, Method, StatusCode, Uri, Version},
    response::Response,
    Router,
};
use sqlx::postgres::PgPoolOptions;
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tower_http::{
    compression::{
        predicate::{DefaultPredicate, Predicate},
        CompressionLayer,
    },
    cors::CorsLayer,
    services::{ServeDir, ServeFile},
    trace::TraceLayer,
};
use tracing_subscriber::EnvFilter;

use crate::{
    config::Config,
    state::{AppState, JwtKeys},
};

#[tokio::main]
async fn main() -> Result<()> {
    // `nerp render-typst <dir>`: render one Typst template in this child process (see
    // crm::docs::typst_engine::render_isolated) and exit.
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("render-typst") {
        std::process::exit(crm::docs::typst_engine::child_main(std::path::Path::new(args.get(2).map(String::as_str).unwrap_or("."))));
    }
    // A line .env cannot parse stops it there, silently dropping every later setting.
    let env_error = dotenvy::dotenv().err().filter(|e| !e.not_found());
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,sqlx=warn,tower_http=info")),
        )
        .init();
    if let Some(e) = env_error {
        tracing::warn!(error = %e, "could not read all of .env; settings after the bad line are ignored (quote values with spaces or < >)");
    }

    let config = Config::from_env()?;
    tokio::fs::create_dir_all(&config.upload_dir)
        .await
        .with_context(|| format!("creating upload dir {}", config.upload_dir.display()))?;

    migrate::ensure_database(&config.database_url).await?;
    let db = PgPoolOptions::new()
        .max_connections(config.database_max_connections)
        .acquire_timeout(Duration::from_secs(10))
        .connect(&config.database_url)
        .await
        .context("connecting to PostgreSQL")?;

    // Migrations (see migrate.rs): one transaction each, one instance at a time.
    migrate::run(&db).await.context("running migrations")?;
    tracing::info!("database migrations are up to date");

    let redis_client = redis::Client::open(config.redis_url.as_str()).context("parsing REDIS_URL")?;
    let redis = redis::aio::ConnectionManager::new(redis_client.clone())
        .await
        .context("connecting to Redis")?;

    let instance_id = format!(
        "{}-{}-{}",
        std::env::var("HOSTNAME").ok().filter(|h| !h.is_empty()).unwrap_or_else(|| "nerp".into()),
        std::process::id(),
        &uuid::Uuid::new_v4().simple().to_string()[..6]
    );
    let state = AppState {
        db,
        redis,
        jwt: Arc::new(JwtKeys::new(&config.jwt_secret)),
        mailer: Arc::new(mail::Mailer::new(&config.smtp).context("configuring SMTP")?),
        hub: Arc::new(realtime::Hub::default()),
        job_wakeup: Arc::new(tokio::sync::Notify::new()),
        instance_id,
        config: Arc::new(config.clone()),
    };

    blob::check_at_start(&state).await?;
    if let Err(e) = export::at_start(&state).await {
        tracing::warn!(error = ?e, "marking interrupted exports failed");
    }
    if routes::admin::redis_persistence(&state).await["persistent"] != serde_json::json!(true) {
        tracing::warn!("Redis keeps nothing across a restart (no appendonly file or snapshots): rate limits and caches start empty after one; see SETUP.md");
    }
    if let Err(e) = auth::tokens::adopt_redis_sessions(&state).await {
        tracing::warn!(error = %e, "could not copy sessions from Redis to the database");
    }
    crm::docs::detect(&state).await;
    crm::docs::seed(&state).await.context("adding built-in document templates")?;

    realtime::start_subscriber(state.clone(), redis_client);
    if config.worker_enabled {
        video::resume(&state).await;
        jobs::start_worker(state.clone());
    }

    let mut app = Router::new()
        .nest("/api", routes::router(state.clone()).layer(api_compression()).layer(axum::middleware::map_response(api_cache_headers)))
        // Calendar apps (CalDAV), outside /api so their well-known address works.
        // Short links of ads (posters, print): /go/{code}.
        .route("/go/{code}", axum::routing::get(tracking::go))
        .route("/.well-known/caldav", axum::routing::any(calendar::dav::well_known))
        .route("/.well-known/carddav", axum::routing::any(calendar::dav::well_known))
        .route("/dav", axum::routing::any(calendar::dav::handle))
        .route("/dav/", axum::routing::any(calendar::dav::handle))
        .route("/dav/{*rest}", axum::routing::any(calendar::dav::handle))
        .layer(TraceLayer::new_for_http());

    if !config.cors_origins.is_empty() {
        let origins: Vec<HeaderValue> = config
            .cors_origins
            .iter()
            .map(|o| HeaderValue::from_str(o))
            .collect::<Result<_, _>>()
            .context("invalid CORS_ORIGINS")?;
        app = app.layer(
            CorsLayer::new()
                .allow_origin(origins)
                .allow_methods([Method::GET, Method::POST, Method::PUT, Method::PATCH, Method::DELETE])
                .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE])
                .allow_credentials(true),
        );
    }

    // Serve the built SPA; unknown paths fall back to index.html for client-side routing.
    if let Some(dir) = &config.static_dir {
        // The build ships .br/.gz copies of every file (adapter precompress), picked by
        // Accept-Encoding, so static files are compressed at no per-request cost.
        let index = ServeFile::new(dir.join("index.html")).precompressed_br().precompressed_gzip();
        let files = ServeDir::new(dir).precompressed_br().precompressed_gzip().fallback(index);
        // Build output under /_app is never answered with index.html: a missing chunk (e.g.
        // an old one after a deploy) must be a 404, not HTML cached for a year.
        let assets = ServeDir::new(dir.join("_app")).precompressed_br().precompressed_gzip();
        app = app.fallback_service(
            Router::new()
                .nest_service("/_app", assets)
                .fallback_service(files)
                .layer(axum::middleware::map_response(static_cache_headers)),
        );
        tracing::info!("serving frontend from {}", dir.display());
    }

    let policy = Arc::new(security_headers::Policy::new(
        config.static_dir.as_deref(),
        config.livekit.as_ref().map(|l| l.url.as_str()),
        config.collabora_url.as_deref(),
        config.cookie_secure,
        &config.public_url,
    ));
    let app = app.layer(axum::middleware::map_response_with_state(policy, security_headers::apply)).with_state(state);
    let listener = tokio::net::TcpListener::bind(config.bind_addr)
        .await
        .with_context(|| format!("binding {}", config.bind_addr))?;
    tracing::info!("NERP listening on http://{}", config.bind_addr);
    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

/// Compression for API responses: JSON and CSV compress ~10x. Formats that are already
/// compressed (PDF, images, office files, archives) are streamed as is.
fn api_compression() -> CompressionLayer<impl Predicate> {
    const PRECOMPRESSED: &[&str] = &[
        "application/pdf", "application/zip", "application/gzip", "application/x-7z", "application/vnd.openxmlformats",
        "application/vnd.oasis.opendocument", "application/octet-stream", "video/", "audio/", "font/woff",
    ];
    let skip_downloads = |_: StatusCode, _: Version, headers: &HeaderMap, _: &Extensions| {
        !headers
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|t| PRECOMPRESSED.iter().any(|p| t.starts_with(p)))
    };
    CompressionLayer::new().compress_when(DefaultPredicate::new().and(skip_downloads))
}

/// API answers are private to whoever asked: no browser or proxy may keep them, unless
/// the route chose its own caching (public pictures, for instance).
async fn api_cache_headers(mut res: Response) -> Response {
    if !res.headers().contains_key(header::CACHE_CONTROL) {
        res.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    res
}

/// Hashed build files never change, so browsers keep them for a year. Everything else
/// (index.html, version.json) is revalidated so a new deploy shows up at once.
async fn static_cache_headers(uri: Uri, mut res: Response) -> Response {
    let value = if uri.path().starts_with("/_app/immutable/") && res.status().is_success() {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    res.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static(value));
    res
}

/// Creates the target database on first start if it does not exist yet.
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("shutting down");
}
