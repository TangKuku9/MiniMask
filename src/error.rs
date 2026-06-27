//! Unified application error type that converts into HTTP responses.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("not found")]
    NotFound,
    #[error("unauthorized")]
    Unauthorized,
    #[error("forbidden")]
    Forbidden,
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    Conflict(String),
    #[error("internal error: {0}")]
    Internal(String),
}

impl AppError {
    pub fn bad_request(msg: impl Into<String>) -> Self {
        AppError::BadRequest(msg.into())
    }
    pub fn internal(msg: impl Into<String>) -> Self {
        AppError::Internal(msg.into())
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = match &self {
            AppError::NotFound => StatusCode::NOT_FOUND,
            AppError::Unauthorized => StatusCode::UNAUTHORIZED,
            AppError::Forbidden => StatusCode::FORBIDDEN,
            AppError::BadRequest(_) => StatusCode::BAD_REQUEST,
            AppError::Conflict(_) => StatusCode::CONFLICT,
            AppError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        let body = Json(json!({ "error": self.to_string() }));
        (status, body).into_response()
    }
}

impl From<anyhow::Error> for AppError {
    fn from(e: anyhow::Error) -> Self {
        AppError::Internal(e.to_string())
    }
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        AppError::Internal(format!("io: {e}"))
    }
}

impl From<serde_json::Error> for AppError {
    fn from(e: serde_json::Error) -> Self {
        AppError::Internal(format!("json: {e}"))
    }
}

pub type AppResult<T> = Result<T, AppError>;
