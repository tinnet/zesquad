//! CSS and htmx, compiled into the binary and served under a versioned path.

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
pub fn base() -> String {
    format!("/static/{}", version::asset_tag())
}

pub async fn serve(Path((tag, path)): Path<(String, String)>) -> Result<Response, AppError> {
    let file = Static::get(&path).ok_or(AppError::NotFound)?;
    let mime = mime_guess::from_path(&path).first_or_octet_stream();
    let content_type = HeaderValue::from_str(mime.as_ref()).map_err(anyhow::Error::from)?;
    let mut response = (
        [(header::CONTENT_TYPE, content_type)],
        file.data.into_owned(),
    )
        .into_response();
    // Debug builds read files from disk on every request; don't let the browser cache them.
    if tag == version::asset_tag() && !cfg!(debug_assertions) {
        let (name, value) = crate::web::cache_forever();
        response.headers_mut().insert(name, value);
    } else {
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    }
    Ok(response)
}
