use axum::body::Body;
use axum::http::{StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use rust_embed::Embed;

#[derive(Embed)]
#[folder = "../../apps/web/dist/"]
struct Assets;

pub async fn spa(uri: Uri) -> Response {
    let requested = uri.path().trim_start_matches('/');
    let path = if requested.is_empty() {
        "index.html"
    } else {
        requested
    };
    if path.contains("..") {
        return StatusCode::BAD_REQUEST.into_response();
    }
    if let Some(file) = Assets::get(path) {
        return asset_response(path, file.data.into_owned());
    }
    if !path.contains('.')
        && let Some(file) = Assets::get("index.html")
    {
        return asset_response("index.html", file.data.into_owned());
    }
    StatusCode::NOT_FOUND.into_response()
}

fn asset_response(path: &str, data: Vec<u8>) -> Response {
    let cache = if path.ends_with("index.html") {
        "no-cache"
    } else {
        "public, max-age=31536000, immutable"
    };
    Response::builder()
        .header(header::CONTENT_TYPE, mime_for(path))
        .header(header::CACHE_CONTROL, cache)
        .body(Body::from(data))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

fn mime_for(path: &str) -> &'static str {
    let extension = path.rsplit('.').next().unwrap_or_default();
    match extension {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "map" => "application/json",
        _ => "application/octet-stream",
    }
}
