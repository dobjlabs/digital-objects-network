//! Embedded default browser UI, served separately from the HTTP API routes.

use axum::{
    Router,
    extract::{Path, State},
    http::{StatusCode, header},
    response::{IntoResponse, Redirect, Response},
    routing::get,
};

include!(concat!(env!("OUT_DIR"), "/bundled_ui.rs"));
type Assets = &'static [(&'static str, &'static [u8])];

pub fn router() -> Router {
    assets_router(BUNDLED_ASSETS)
}

fn assets_router(assets: Assets) -> Router {
    Router::new()
        .route("/", get(|| async { Redirect::temporary("/ui/") }))
        .route("/ui", get(|| async { Redirect::temporary("/ui/") }))
        .route("/ui/", get(index))
        .route("/ui/{*path}", get(asset))
        .with_state(assets)
}

async fn index(State(assets): State<Assets>) -> Response {
    serve(assets, "index.html").await
}

async fn asset(State(assets): State<Assets>, Path(path): Path<String>) -> Response {
    // Path is already percent-decoded by axum. Reject Windows separators,
    // drive prefixes, traversal and hidden files on every platform.
    if path.contains(['\\', ':', '\0'])
        || path.split('/').any(|part| part.starts_with('.'))
        || path.starts_with('/')
    {
        return StatusCode::NOT_FOUND.into_response();
    }
    let name = if path.ends_with('/') {
        format!("{path}index.html")
    } else {
        path
    };
    serve(assets, &name).await
}

async fn serve(assets: Assets, name: &str) -> Response {
    let asset = |name: &str| {
        assets
            .iter()
            .find(|(path, _)| *path == name)
            .map(|(_, bytes)| *bytes)
    };
    let (name, bytes) = match asset(name) {
        Some(bytes) => (name, bytes),
        None if name == "index.html" || std::path::Path::new(name).extension().is_none() => {
            // Keep SPA fallback confined to /ui/ and preserve missing-asset errors.
            match asset("index.html") {
                Some(bytes) => ("index.html", bytes),
                None => return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "This dobjd was built without a UI. Use Vite for development, or rebuild with --features bundled-ui after building interfaces/gui.",
                ).into_response(),
            }
        }
        None => return StatusCode::NOT_FOUND.into_response(),
    };
    let bytes = if name == "index.html" {
        // Only the daemon-served index selects the daemon's own API origin.
        // The standalone build has no such marker and defaults to port 7717.
        String::from_utf8_lossy(bytes)
            .replacen(
                "<head>",
                "<head><base href=\"/ui/\"><meta name=\"dobjd-api-url\" content=\"/\">",
                1,
            )
            .into_bytes()
    } else {
        bytes.to_vec()
    };
    (
        [
            (header::CONTENT_TYPE, content_type(name)),
            (header::CACHE_CONTROL, "no-cache"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (header::CONTENT_SECURITY_POLICY, "frame-ancestors 'none'"),
            (header::X_FRAME_OPTIONS, "DENY"),
        ],
        bytes,
    )
        .into_response()
}

fn content_type(name: &str) -> &'static str {
    match name.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        "wasm" => "application/wasm",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use tower::ServiceExt;

    static ASSETS: &[(&str, &[u8])] = &[
        ("index.html", b"<html>default</html>"),
        ("assets/app.js", b"console.log('default')"),
    ];

    async fn request(assets: Assets, path: &str) -> Response {
        assets_router(assets)
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn compiled_ui_matches_the_selected_build_feature() {
        let source = BUNDLED_ASSETS;
        let response = request(source, "/ui/").await;
        if cfg!(feature = "bundled-ui") {
            assert_eq!(response.status(), StatusCode::OK);
            let html = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            let html = std::str::from_utf8(&html).unwrap();
            assert!(html.contains("name=\"dobjd-api-url\" content=\"/\""));
            assert!(html.contains("<base href=\"/ui/\">"));
            let mut asset_count = 0;
            for (name, bytes) in BUNDLED_ASSETS {
                if name.starts_with("assets/") {
                    let path = format!("/ui/{name}");
                    if html.contains(name) {
                        let response = request(source, &path).await;
                        assert_eq!(response.status(), StatusCode::OK);
                        assert_eq!(
                            to_bytes(response.into_body(), usize::MAX).await.unwrap(),
                            *bytes
                        );
                        asset_count += 1;
                    }
                }
            }
            assert!(asset_count > 0, "built index must reference bundled assets");
        } else {
            assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        }
    }

    #[tokio::test]
    async fn ui_documents_cannot_be_embedded_by_another_page() {
        for path in ["/ui/", "/ui/index.html", "/ui/profile"] {
            let response = request(ASSETS, path).await;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(
                response.headers()[header::CONTENT_SECURITY_POLICY],
                "frame-ancestors 'none'",
                "{path}"
            );
            assert_eq!(
                response.headers()[header::X_FRAME_OPTIONS],
                "DENY",
                "{path}"
            );
        }
    }

    #[tokio::test]
    async fn missing_ui_explains_navigation_failures_but_preserves_asset_errors() {
        for path in [
            "/ui/",
            "/ui/index.html",
            "/ui/profile",
            "/ui/settings/nested",
        ] {
            let response = request(&[], path).await;
            assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE, "{path}");
            let body = to_bytes(response.into_body(), 1024).await.unwrap();
            assert!(String::from_utf8_lossy(&body).contains("built without a UI"));
        }
        for path in [
            "/ui/missing.js",
            "/ui/.hidden",
            "/ui/invalid:path",
            "/objects",
        ] {
            assert_eq!(
                request(&[], path).await.status(),
                StatusCode::NOT_FOUND,
                "{path}"
            );
        }
    }

    #[tokio::test]
    async fn bundled_assets_and_spa_routes_do_not_swallow_api_or_asset_errors() {
        let source = ASSETS;
        let response = request(source, "/ui/assets/app.js").await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/javascript; charset=utf-8"
        );
        assert_eq!(
            to_bytes(response.into_body(), 1024).await.unwrap(),
            "console.log('default')"
        );
        assert_eq!(
            request(source, "/ui/profile").await.status(),
            StatusCode::OK
        );
        assert_eq!(
            request(source, "/ui/missing.js").await.status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            request(source, "/objects").await.status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            request(source, "/ui").await.headers()[header::LOCATION],
            "/ui/"
        );
    }
}
