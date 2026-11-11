use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

pub type AppResult<T> = Result<T, AppError>;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0}")]
    BadRequest(String),
    #[error("authentication required")]
    Unauthorized,
    #[error("{0}")]
    InvalidCredentials(String),
    #[error("you do not have permission to do this")]
    Forbidden,
    #[error("{0} not found")]
    NotFound(&'static str),
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    TooManyRequests(String),
    #[error("{0}")]
    PayloadTooLarge(String),
    /// Refused by a company rule (an IP address outside the allowed ranges).
    #[error("{0}")]
    Blocked(String),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error(transparent)]
    Redis(#[from] redis::RedisError),
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

impl AppError {
    pub fn bad(msg: impl Into<String>) -> Self {
        Self::BadRequest(msg.into())
    }
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        Self::Internal(e.into())
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, code) = match &self {
            AppError::BadRequest(_) => (StatusCode::BAD_REQUEST, "bad_request"),
            AppError::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized"),
            AppError::InvalidCredentials(_) => (StatusCode::UNAUTHORIZED, "invalid_credentials"),
            AppError::Forbidden => (StatusCode::FORBIDDEN, "forbidden"),
            AppError::Blocked(_) => (StatusCode::FORBIDDEN, "blocked"),
            AppError::NotFound(_) => (StatusCode::NOT_FOUND, "not_found"),
            AppError::Conflict(_) => (StatusCode::CONFLICT, "conflict"),
            AppError::TooManyRequests(_) => (StatusCode::TOO_MANY_REQUESTS, "too_many_requests"),
            AppError::PayloadTooLarge(_) => (StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large"),
            AppError::Database(sqlx::Error::RowNotFound) => (StatusCode::NOT_FOUND, "not_found"),
            AppError::Database(e) if is_unique_violation(e) => (StatusCode::CONFLICT, "conflict"),
            AppError::Database(e) if is_bad_text(e) => (StatusCode::BAD_REQUEST, "bad_request"),
            AppError::Database(_) | AppError::Redis(_) | AppError::Internal(_) => {
                (StatusCode::INTERNAL_SERVER_ERROR, "internal")
            }
        };
        let message = match (&self, status) {
            (_, StatusCode::INTERNAL_SERVER_ERROR) => {
                tracing::error!(error = ?self, "internal error");
                "internal server error".to_string()
            }
            (AppError::Database(sqlx::Error::RowNotFound), _) => "not found".to_string(),
            (AppError::Database(_), StatusCode::CONFLICT) => "record already exists".to_string(),
            (AppError::Database(_), StatusCode::BAD_REQUEST) => "text can't contain control characters such as NUL".to_string(),
            _ => self.to_string(),
        };
        (status, Json(json!({ "error": { "code": code, "message": message } }))).into_response()
    }
}

pub fn is_unique_violation(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(db) if db.code().as_deref() == Some("23505"))
}

/// Text the database refuses (a NUL character, invalid UTF-8): the request's fault.
pub fn is_bad_text(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(db) if matches!(db.code().as_deref(), Some("22021") | Some("22P05")))
}

pub fn is_fk_violation(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(db) if db.code().as_deref() == Some("23503"))
}
