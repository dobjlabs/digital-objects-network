use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use driver::DriverError;
use serde::Serialize;

/// Error type returned by HTTP route handlers.
///
/// Wraps `anyhow::Error` so handler bodies can use `?` against the existing
/// driver API surface, and renders as `{"error": "<message>"}` JSON. Status
/// code defaults to 500, but `From<anyhow::Error>` peeks inside for a
/// [`DriverError`] and maps known-cause variants (missing object, unknown
/// action, malformed upload, …) to 404 / 400.
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

fn status_for_driver_error(err: &DriverError) -> StatusCode {
    match err {
        DriverError::UnknownAction(_)
        | DriverError::UnknownClass(_)
        | DriverError::ObjectNotFound(_)
        | DriverError::ObjectFileNotFound(_) => StatusCode::NOT_FOUND,
        DriverError::InvalidInput(_) => StatusCode::BAD_REQUEST,
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(err: anyhow::Error) -> Self {
        let status = err
            .downcast_ref::<DriverError>()
            .map(status_for_driver_error)
            .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        ApiError::new(status, err.to_string())
    }
}

impl From<DriverError> for ApiError {
    fn from(err: DriverError) -> Self {
        ApiError::new(status_for_driver_error(&err), err.to_string())
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
