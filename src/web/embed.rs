//! Embedded SPA static asset serving via rust-embed. Unknown paths fall back to
//! `index.html` so client-side routing works.

use axum::body::Body;
use axum::extract::Request;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "web-ui/dist/"]
struct Asset;

pub async fn serve_embed(req: Request) -> Response {
    let full = req.uri().path();
    // Unknown API paths should 404, not fall back to the SPA.
    if full.starts_with("/api/") {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    }
    let req_path = full.trim_start_matches('/');
    let lookup = if req_path.is_empty() { "index.html" } else { req_path };

    // Exact asset match.
    if let Some(file) = Asset::get(lookup) {
        return make_response(file.data.into_owned(), lookup);
    }
    // SPA fallback: serve index.html for client-side routes.
    if let Some(file) = Asset::get("index.html") {
        return make_response(file.data.into_owned(), "index.html");
    }
    (StatusCode::NOT_FOUND, "not found").into_response()
}

fn make_response(data: Vec<u8>, path_for_mime: &str) -> Response {
    let mime = mime_guess::from_path(path_for_mime).first_or_octet_stream();
    let mut resp = (StatusCode::OK, Body::from(data)).into_response();
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        mime.as_ref().parse().unwrap_or_else(|_| "application/octet-stream".parse().unwrap()),
    );
    // Never cache index.html so new deploys are picked up; assets are hashed.
    if path_for_mime == "index.html" {
        resp.headers_mut().insert(
            header::CACHE_CONTROL,
            "no-cache, no-store, must-revalidate".parse().unwrap(),
        );
    }
    resp
}
