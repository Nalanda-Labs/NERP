//! The public API: personal API keys (so other systems can call the same REST API the app
//! uses), outgoing event webhooks, and the API reference.

pub mod keys;
pub mod webhooks;

use axum::{routing::get, Json, Router};
use serde_json::Value;

use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .merge(keys::router())
        .merge(webhooks::router())
        .route("/openapi.json", get(openapi))
}

/// The API reference (OpenAPI 3.1) for the public endpoints.
async fn openapi() -> Json<Value> {
    Json(serde_json::from_str(include_str!("../../assets/openapi.json")).expect("assets/openapi.json is valid JSON"))
}
