use std::net::{IpAddr, SocketAddr};

use axum::{
    extract::{ConnectInfo, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::{
    config::AppConfig,
    db,
    types::{
        decode_cursor, encode_cursor, CreatePostRequest, CreateResponseRequest, Cursor,
        ListPostsQuery, ListPostsResponse,
    },
};

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub max_page_size: u32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ErrorResponse {
    error: String,
}

pub enum ApiError {
    BadRequest(String),
    NotFound(String),
    Internal,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            ApiError::BadRequest(message) => (StatusCode::BAD_REQUEST, message),
            ApiError::NotFound(message) => (StatusCode::NOT_FOUND, message),
            ApiError::Internal => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal server error".to_string(),
            ),
        };

        (status, Json(ErrorResponse { error: message })).into_response()
    }
}

pub fn build_router(state: AppState, cfg: &AppConfig) -> Result<Router, anyhow::Error> {
    use axum::http::Method;
    use tower_http::cors::{Any, CorsLayer};

    let cors = if cfg.cors_origin.trim() == "*" {
        CorsLayer::new()
            .allow_origin(Any)
            .allow_methods([Method::GET, Method::POST])
            .allow_headers(Any)
    } else {
        let origin: axum::http::HeaderValue = cfg.cors_origin.parse()?;
        CorsLayer::new()
            .allow_origin(origin)
            .allow_methods([Method::GET, Method::POST])
            .allow_headers(Any)
    };

    Ok(Router::new()
        .route("/api/v1/posts", get(get_posts).post(create_post))
        .route("/api/v1/posts/{post_id}/responses", post(create_response))
        .with_state(state)
        .layer(cors))
}

pub async fn get_posts(
    State(state): State<AppState>,
    Query(query): Query<ListPostsQuery>,
) -> Result<Json<ListPostsResponse>, ApiError> {
    let limit = query.limit.unwrap_or(20).max(1).min(state.max_page_size);
    let cursor = match query.cursor {
        Some(raw) => Some(
            decode_cursor(&raw)
                .ok_or_else(|| ApiError::BadRequest("invalid cursor".to_string()))?,
        ),
        None => None,
    };

    let items = db::list_posts(
        &state.pool,
        limit,
        cursor,
        query.q.as_deref(),
        query.live_only.unwrap_or(false),
    )
    .await
    .map_err(|_| ApiError::Internal)?;

    let next_cursor = if items.len() as u32 == limit {
        items.last().map(|last| {
            encode_cursor(Cursor {
                created_at: last.time,
                id: last.id,
            })
        })
    } else {
        None
    };

    Ok(Json(ListPostsResponse { items, next_cursor }))
}

pub async fn create_post(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(request): Json<CreatePostRequest>,
) -> Result<Json<crate::types::PostDto>, ApiError> {
    validate_post_request(&request)?;
    let author_ip = extract_author_ip(&headers, addr.ip());
    let post = db::create_post(&state.pool, &author_ip, request)
        .await
        .map_err(|_| ApiError::Internal)?;
    Ok(Json(post))
}

pub async fn create_response(
    State(state): State<AppState>,
    Path(post_id): Path<Uuid>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(request): Json<CreateResponseRequest>,
) -> Result<Json<crate::types::ResponseDto>, ApiError> {
    validate_response_request(&request)?;
    let author_ip = extract_author_ip(&headers, addr.ip());
    let response = db::create_response(&state.pool, post_id, &author_ip, request)
        .await
        .map_err(|_| ApiError::Internal)?;

    match response {
        Some(value) => Ok(Json(value)),
        None => Err(ApiError::NotFound("post not found".to_string())),
    }
}

fn validate_post_request(request: &CreatePostRequest) -> Result<(), ApiError> {
    if request.title.trim().is_empty() || request.title.chars().count() > 200 {
        return Err(ApiError::BadRequest(
            "title must be between 1 and 200 characters".to_string(),
        ));
    }

    if request.description.trim().is_empty() || request.description.chars().count() > 5000 {
        return Err(ApiError::BadRequest(
            "description must be between 1 and 5000 characters".to_string(),
        ));
    }

    validate_claims(&request.claims)
}

fn validate_response_request(request: &CreateResponseRequest) -> Result<(), ApiError> {
    if request.description.trim().is_empty() || request.description.chars().count() > 5000 {
        return Err(ApiError::BadRequest(
            "description must be between 1 and 5000 characters".to_string(),
        ));
    }
    validate_claims(&request.claims)
}

fn validate_claims(claims: &[crate::types::Claim]) -> Result<(), ApiError> {
    if claims.len() > 64 {
        return Err(ApiError::BadRequest(
            "claims must contain at most 64 items".to_string(),
        ));
    }

    for claim in claims {
        if claim.name.trim().is_empty() || claim.name.chars().count() > 200 {
            return Err(ApiError::BadRequest(
                "claim name must be between 1 and 200 characters".to_string(),
            ));
        }
        if claim.hash.trim().is_empty() || claim.hash.chars().count() > 256 {
            return Err(ApiError::BadRequest(
                "claim hash must be between 1 and 256 characters".to_string(),
            ));
        }
    }

    Ok(())
}

fn extract_author_ip(headers: &HeaderMap, fallback: IpAddr) -> String {
    let forwarded = headers
        .get("x-forwarded-for")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .map(str::trim)
        .and_then(|value| value.parse::<IpAddr>().ok());

    forwarded.unwrap_or(fallback).to_string()
}
