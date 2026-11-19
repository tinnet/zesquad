//! CSS and htmx, compiled into the binary and served under a versioned path.

use std::sync::LazyLock;

use axum::body::Body;
use axum::extract::Path;
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};
use rust_embed::Embed;

use crate::version;
use crate::web::AppError;

#[derive(Embed)]
#[folder = "static/"]
struct Static;

/// URL prefix for static files. It changes with every version, so a new release never gets
/// stale CSS from a browser cache.
#[must_use]
pub fn base() -> &'static str {
    static BASE: LazyLock<String> = LazyLock::new(|| format!("/static/{}", version::asset_tag()));
    &BASE
}

pub async fn serve(Path((tag, path)): Path<(String, String)>) -> Result<Response, AppError> {
    let file = Static::get(&path).ok_or(AppError::NotFound)?;
    let mime = mime_guess::from_path(&path).first_or_octet_stream();
    let content_type = HeaderValue::from_str(mime.as_ref()).map_err(anyhow::Error::from)?;
    let mut response = (
        [(header::CONTENT_TYPE, content_type)],
        // Borrowed from the binary in release builds, so no copy.
        Body::from(file.data),
    )
        .into_response();
    // A versioned path never changes, so browsers may keep it for a year. Debug builds read
    // files from disk on every request; don't let the browser cache them.
    let cache = if tag == version::asset_tag() && !cfg!(debug_assertions) {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    Ok(response)
}
