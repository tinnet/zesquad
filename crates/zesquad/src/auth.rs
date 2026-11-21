//! The optional shared password in front of the whole app.
//!
//! With `ZESQUAD_PASSWORD` set, every page needs a session cookie, which the login page sets.
//! The cookie holds `sha256(key, password)`, where the key is random and stored in the
//! database: it survives restarts, isn't the password, and changing the password logs everyone
//! out. Health check, version and static files stay open so containers and the login page work;
//! `web::router` keeps those routes outside this middleware.

use std::fmt::Write as _;
use std::time::Duration;

use axum::extract::{Request, State};
use axum::http::uri::PathAndQuery;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};
use sha2::{Digest as _, Sha256};
use sqlx::SqlitePool;

use crate::web::{AppState, Htmx};

const COOKIE: &str = "zesquad_session";
/// Slows down guessing; one wrong password costs the guesser this long.
const WRONG_PASSWORD_DELAY: Duration = Duration::from_millis(750);

pub struct Auth {
    password_hash: [u8; 32],
    token: String,
}

impl Auth {
    /// Loads (or creates on first start) the cookie key and derives the session token.
    ///
    /// # Errors
    ///
    /// On database errors.
    pub async fn new(db: &SqlitePool, password: &str) -> anyhow::Result<Self> {
        let candidate = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        sqlx::query(
            "INSERT INTO settings (name, value) VALUES ('session_key', ?) ON CONFLICT DO NOTHING",
        )
        .bind(&candidate)
        .execute(db)
        .await?;
        let key: String =
            sqlx::query_scalar("SELECT value FROM settings WHERE name = 'session_key'")
                .fetch_one(db)
                .await?;
        Ok(Self {
            password_hash: Sha256::digest(password.as_bytes()).into(),
            token: hex(&Sha256::new()
                .chain_update(key.as_bytes())
                .chain_update([0])
                .chain_update(password.as_bytes())
                .finalize()),
        })
    }

    /// Compares hashes so the time taken doesn't depend on how much of the password was right.
    #[must_use]
    pub fn password_matches(&self, given: &str) -> bool {
        let given: [u8; 32] = Sha256::digest(given.as_bytes()).into();
        constant_time_eq(&given, &self.password_hash)
    }

    fn has_session(&self, headers: &HeaderMap) -> bool {
        cookie(headers, COOKIE)
            .is_some_and(|v| constant_time_eq(v.as_bytes(), self.token.as_bytes()))
    }

    /// The `Set-Cookie` value after a successful login.
    #[must_use]
    pub fn session_cookie(&self, headers: &HeaderMap) -> String {
        format!(
            "{COOKIE}={}; Path=/; HttpOnly; SameSite=Lax; Max-Age=31536000{}",
            self.token,
            secure(headers)
        )
    }
}

/// `; Secure` when the request came over HTTPS through a proxy, for the end of a `Set-Cookie`;
/// plain-HTTP local runs would lose the cookie otherwise.
#[must_use]
pub fn secure(headers: &HeaderMap) -> &'static str {
    let https = headers
        .get("x-forwarded-proto")
        .is_some_and(|v| v.as_bytes().eq_ignore_ascii_case(b"https"));
    if https { "; Secure" } else { "" }
}

/// Clears the session cookie.
#[must_use]
pub fn logout_cookie() -> String {
    format!("{COOKIE}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0")
}

pub async fn wrong_password_delay() {
    tokio::time::sleep(WRONG_PASSWORD_DELAY).await;
}

/// Middleware: lets requests with a session through, sends everyone else to the login page.
pub async fn require(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let Some(auth) = &state.auth else {
        return next.run(request).await;
    };
    if auth.has_session(request.headers()) {
        return next.run(request).await;
    }
    let login = format!(
        "/login?next={}",
        crate::web::urlencode(&return_to(&request))
    );
    if Htmx::of(request.headers()) {
        // htmx would swap a redirect's target into the page; HX-Redirect makes it navigate
        // instead. htmx only acts on it for responses it processes, so 2xx is the safe choice.
        let mut response = StatusCode::OK.into_response();
        if let Ok(value) = HeaderValue::from_str(&login) {
            response.headers_mut().insert("hx-redirect", value);
        }
        return response;
    }
    Redirect::to(&login).into_response()
}

/// The page to come back to after logging in. A GET is the page itself. An htmx request is a
/// fragment or a form post, so it's the page htmx was on (`HX-Current-URL`); any other form
/// post can't be repeated by a GET, so it's the page it came from (`Referer`), or `/`.
fn return_to(request: &Request) -> String {
    let uri = request.uri();
    if request.method() == Method::GET && !Htmx::of(request.headers()) {
        return uri
            .path_and_query()
            .map_or("/", PathAndQuery::as_str)
            .to_owned();
    }
    let page = ["hx-current-url", header::REFERER.as_str()]
        .iter()
        .find_map(|name| request.headers().get(*name)?.to_str().ok())
        .and_then(|url| url.parse::<Uri>().ok())
        .and_then(|url| url.path_and_query().map(|p| p.as_str().to_owned()));
    zesquad_core::redirect::safe_next(page.as_deref()).to_owned()
}

#[must_use]
pub fn cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find_map(|(k, v)| (k == name).then_some(v))
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_cookies() {
        let mut h = HeaderMap::new();
        h.append(
            header::COOKIE,
            HeaderValue::from_static("a=1; zesquad_session=xyz"),
        );
        assert_eq!(cookie(&h, COOKIE), Some("xyz"));
        assert_eq!(cookie(&h, "b"), None);
    }

    #[test]
    fn compares_equal_lengths_only() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
    }
}
