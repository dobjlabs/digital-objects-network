use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;

/// Error type returned by HTTP route handlers.
///
/// Wraps `anyhow::Error` so handler bodies can use `?` against the existing
/// driver API surface, and renders as `{"error": "<message>"}` JSON with a
/// 500 status by default.
#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    pub fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(err: anyhow::Error) -> Self {
        ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
    }
}

impl From<std::io::Error> for ApiError {
    fn from(err: std::io::Error) -> Self {
        ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
    }
}

#[derive(Serialize)]
struct ErrorBody<'a> {
    error: &'a str,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = Json(ErrorBody {
            error: &self.message,
        });
        (self.status, body).into_response()
    }
}

pub type ApiResult<T> = Result<T, ApiError>;
