//! The optional shared password in front of the whole app.
//!
//! With `ZESQUAD_PASSWORD` set, every page needs a session cookie, which the login page sets.
//! The cookie holds `sha256(key, password)`, where the key is random and stored in the
//! database: it survives restarts, isn't the password, and changing the password logs everyone
//! out. Health check, version and static files stay open so containers and the login page work.

use std::fmt::Write as _;
use std::time::Duration;

use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};
use sha2::{Digest as _, Sha256};
use sqlx::SqlitePool;

use crate::web::AppState;

pub const COOKIE: &str = "zesquad_session";
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

    /// The `Set-Cookie` value after a successful login. `Secure` when the request came over
    /// HTTPS through a proxy; plain-HTTP local runs would lose the cookie otherwise.
    #[must_use]
    pub fn session_cookie(&self, headers: &HeaderMap) -> String {
        let secure = headers
            .get("x-forwarded-proto")
            .is_some_and(|v| v.as_bytes().eq_ignore_ascii_case(b"https"));
        format!(
            "{COOKIE}={}; Path=/; HttpOnly; SameSite=Lax; Max-Age=31536000{}",
            self.token,
            if secure { "; Secure" } else { "" }
        )
    }
}

/// Clears the session cookie.
#[must_use]
pub fn logout_cookie() -> String {
    format!("{COOKIE}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0")
}

pub async fn wrong_password_delay() {
    tokio::time::sleep(WRONG_PASSWORD_DELAY).await;
}

fn is_open(path: &str) -> bool {
    // Logging out needs no session: it only clears the cookie.
    matches!(path, "/login" | "/logout" | "/healthz" | "/version") || path.starts_with("/static/")
}

/// Middleware: lets requests with a session through, sends everyone else to the login page.
pub async fn require(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let Some(auth) = &state.auth else {
        return next.run(request).await;
    };
    if is_open(request.uri().path()) || auth.has_session(request.headers()) {
        return next.run(request).await;
    }
    let target = request
        .uri()
        .path_and_query()
        .map_or("/", axum::http::uri::PathAndQuery::as_str);
    let login = format!("/login?next={}", crate::web::urlencode(target));
    if request.headers().contains_key("hx-request") {
        // htmx would swap a redirect's target into the page; this makes it navigate instead.
        let mut response = StatusCode::UNAUTHORIZED.into_response();
        if let Ok(value) = HeaderValue::from_str(&login) {
            response.headers_mut().insert("hx-redirect", value);
        }
        // htmx only acts on HX-Redirect for responses it processes; 2xx is the safe choice.
        *response.status_mut() = StatusCode::OK;
        return response;
    }
    Redirect::to(&login).into_response()
}

/// Only same-site paths, so the login page can't be used to send people elsewhere.
#[must_use]
pub fn safe_next(next: Option<&str>) -> &str {
    match next {
        Some(n) if n.starts_with('/') && !n.starts_with("//") && !n.contains('\\') => n,
        _ => "/",
    }
}

fn cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
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
    fn next_stays_on_site() {
        assert_eq!(safe_next(Some("/s/abc?tag=raid")), "/s/abc?tag=raid");
        assert_eq!(safe_next(Some("//evil.example")), "/");
        assert_eq!(safe_next(Some("https://evil.example")), "/");
        assert_eq!(safe_next(Some("/\\evil.example")), "/");
        assert_eq!(safe_next(None), "/");
    }

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
    fn open_paths() {
        assert!(is_open("/healthz") && is_open("/login") && is_open("/logout"));
        assert!(is_open("/static/x/style.css"));
        assert!(!is_open("/") && !is_open("/s/abc") && !is_open("/loginx"));
    }

    #[test]
    fn compares_equal_lengths_only() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
    }
}
