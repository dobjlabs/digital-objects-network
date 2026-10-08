//! Browser origin restrictions are not authentication. All local frontends
//! are trusted. Host validation also prevents DNS-rebound domains from
//! addressing this API.

use axum::{
    extract::{Request, State},
    http::{HeaderValue, StatusCode, Uri, header, uri::Authority},
    middleware::Next,
    response::{IntoResponse, Response},
};
use tower_http::cors::{AllowOrigin, Any, CorsLayer};

#[derive(Clone)]
pub struct WebAccess {
    port: u16,
}

impl WebAccess {
    pub fn new(port: u16) -> Self {
        Self { port }
    }

    fn allows_origin(&self, origin: &HeaderValue) -> bool {
        let Ok(origin) = origin.to_str() else {
            return false;
        };
        let Ok(uri) = origin.parse::<Uri>() else {
            return false;
        };
        matches!(uri.scheme_str(), Some("http" | "https"))
            && uri
                .authority()
                .is_some_and(|a| is_loopback_host(a.host()) && !a.as_str().contains('@'))
            && uri.path() == "/"
            && uri.query().is_none()
            && !origin.ends_with('/')
    }

    pub fn cors(&self) -> CorsLayer {
        let access = self.clone();
        CorsLayer::new()
            .allow_origin(AllowOrigin::predicate(move |origin, _| {
                access.allows_origin(origin)
            }))
            .allow_methods(Any)
            .allow_headers(Any)
    }

    fn allows_request(&self, request: &Request) -> bool {
        if request.headers().get_all(header::HOST).iter().count() > 1
            || request.headers().get_all(header::ORIGIN).iter().count() > 1
        {
            return false;
        }
        let authority = request
            .headers()
            .get(header::HOST)
            .and_then(|host| host.to_str().ok())
            .and_then(|host| host.parse::<Authority>().ok())
            .or_else(|| request.uri().authority().cloned());
        let host_allowed = authority.is_some_and(|host| {
            is_loopback_host(host.host())
                && !host.as_str().contains('@')
                && match host.port() {
                    Some(port) => port.as_u16() == self.port,
                    None => self.port == 80,
                }
        });
        host_allowed
            && request
                .headers()
                .get(header::ORIGIN)
                .is_none_or(|origin| self.allows_origin(origin))
    }
}

fn is_loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost") || matches!(host, "127.0.0.1" | "[::1]")
}

pub async fn guard(State(access): State<WebAccess>, request: Request, next: Next) -> Response {
    if !access.allows_request(&request) {
        return (
            StatusCode::FORBIDDEN,
            "Browser origin or host is not allowed by dobjd",
        )
            .into_response();
    }
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, body::Body, middleware, routing::post};
    use tower::ServiceExt;

    #[tokio::test]
    async fn rejects_untrusted_writes_and_rebound_hosts_before_handler_runs() {
        let access = WebAccess::new(7717);
        let app = Router::new()
            .route("/write", post(|| async { StatusCode::ACCEPTED }))
            .layer(access.cors())
            .layer(middleware::from_fn_with_state(access, guard));
        for (host, origin, expected) in [
            ("127.0.0.1:7717", None, StatusCode::ACCEPTED),
            (
                "localhost:7717",
                Some("http://localhost:5173"),
                StatusCode::ACCEPTED,
            ),
            (
                "127.0.0.1:7717",
                Some("http://127.0.0.1:8000"),
                StatusCode::ACCEPTED,
            ),
            (
                "127.0.0.1:7717",
                Some("https://attacker.example"),
                StatusCode::FORBIDDEN,
            ),
            ("127.0.0.1:7717", Some("null"), StatusCode::FORBIDDEN),
            (
                "rebound.example:7717",
                Some("http://rebound.example:7717"),
                StatusCode::FORBIDDEN,
            ),
        ] {
            let mut request = Request::builder()
                .method("POST")
                .uri("/write")
                .header(header::HOST, host);
            if let Some(origin) = origin {
                request = request.header(header::ORIGIN, origin);
            }
            let response = app
                .clone()
                .oneshot(request.body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), expected, "{host} {origin:?}");
            if expected == StatusCode::ACCEPTED
                && let Some(origin) = origin
            {
                assert_eq!(
                    response.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN],
                    origin
                );
            }
        }
    }

    #[tokio::test]
    async fn local_frontend_preflight_is_allowed() {
        let access = WebAccess::new(7717);
        let app = Router::new()
            .route("/write", post(|| async { StatusCode::ACCEPTED }))
            .layer(access.cors())
            .layer(middleware::from_fn_with_state(access, guard));
        let response = app
            .oneshot(
                Request::builder()
                    .method("OPTIONS")
                    .uri("/write")
                    .header(header::HOST, "127.0.0.1:7717")
                    .header(header::ORIGIN, "http://localhost:5173")
                    .header(header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
                    .header(header::ACCESS_CONTROL_REQUEST_HEADERS, "content-type")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(response.status().is_success());
        assert_eq!(
            response.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN],
            "http://localhost:5173"
        );
    }
}
