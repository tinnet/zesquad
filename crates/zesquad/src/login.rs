//! Logging in with Battle.net (OAuth authorization code flow) and the sessions that follow.
//!
//! zesquad asks for the `openid` scope only, which gives the Battle.net account id and
//! BattleTag and nothing else. The player's token is used once, to ask `/userinfo`, and thrown
//! away. See `docs/battlenet-api.md`.
//!
//! A session is a random token in an `HttpOnly`, `SameSite=Lax` cookie; the database keeps only
//! its SHA-256. Sessions last 30 days and are extended when used in their second half.

use std::time::Duration;

use anyhow::{Context as _, bail};
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::middleware::Next;
use axum::response::Response;
use reqwest::Url;
use sha2::{Digest as _, Sha256};
use zesquad_core::account::{Identity, parse_userinfo};

use crate::db;
use crate::web::AppState;

/// Battle.net's OAuth host: `/authorize` and `/userinfo` for every region but China.
pub const OAUTH_URL: &str = "https://oauth.battle.net";

/// The session cookie.
pub const COOKIE: &str = "zesquad_user";
/// Carries a login's state and where to return to, while away at Battle.net.
pub const LOGIN_COOKIE: &str = "zesquad_login";

const SESSION_DAYS: i64 = 30;
const DAY: i64 = 86_400;

/// Where and as whom to log people in.
#[derive(Debug, Clone)]
pub struct Config {
    pub client_id: String,
    pub client_secret: String,
    /// `/authorize` and `/userinfo` live here (tests point it at a fake).
    pub oauth_url: Url,
    pub token_url: Url,
    /// Where zesquad is reached, e.g. `https://zesquad.example`; Battle.net sends people back
    /// to `<public_url>/auth/callback`, which must be registered with the API client.
    pub public_url: Url,
}

pub struct Login {
    http: reqwest::Client,
    config: Config,
}

impl Login {
    /// # Errors
    ///
    /// When the HTTP client can't be built (TLS setup).
    pub fn new(config: Config) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(format!("zesquad/{}", crate::version::short()))
            .timeout(Duration::from_secs(20))
            .build()?;
        Ok(Self { http, config })
    }

    fn redirect_uri(&self) -> String {
        format!(
            "{}/auth/callback",
            self.config.public_url.as_str().trim_end_matches('/')
        )
    }

    /// Cookies get `Secure` when zesquad is reached over HTTPS.
    #[must_use]
    pub fn secure(&self) -> &'static str {
        if self.config.public_url.scheme() == "https" {
            "; Secure"
        } else {
            ""
        }
    }

    /// Battle.net's login page, asking for nothing but who someone is.
    ///
    /// # Errors
    ///
    /// When the OAuth URL can't be joined (it's checked at startup, so never in practice).
    pub fn authorize_url(&self, state: &str) -> anyhow::Result<Url> {
        let mut url = self.config.oauth_url.join("authorize")?;
        url.query_pairs_mut()
            .append_pair("response_type", "code")
            .append_pair("client_id", &self.config.client_id)
            .append_pair("scope", "openid")
            .append_pair("state", state)
            .append_pair("redirect_uri", &self.redirect_uri());
        Ok(url)
    }

    /// Swaps the code Battle.net sent back for a token and asks who it belongs to.
    ///
    /// # Errors
    ///
    /// When Battle.net can't be reached, refuses the code, or its answer can't be read.
    pub async fn identify(&self, code: &str) -> anyhow::Result<Identity> {
        #[derive(serde::Deserialize)]
        struct Token {
            access_token: String,
        }
        let response = self
            .http
            .post(self.config.token_url.clone())
            .basic_auth(&self.config.client_id, Some(&self.config.client_secret))
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", &self.redirect_uri()),
            ])
            .send()
            .await
            .context("asking Battle.net for a player token")?;
        if !response.status().is_success() {
            bail!("Battle.net refused the login code: {}", response.status());
        }
        let token: Token = response.json().await.context("reading the player token")?;
        let response = self
            .http
            .get(self.config.oauth_url.join("userinfo")?)
            .bearer_auth(&token.access_token)
            .send()
            .await
            .context("asking Battle.net who logged in")?;
        if !response.status().is_success() {
            bail!("Battle.net refused /userinfo: {}", response.status());
        }
        let info: serde_json::Value = response.json().await.context("reading /userinfo")?;
        parse_userinfo(&info).context("/userinfo has no account id or BattleTag")
    }
}

/// A new random token: 244 random bits from two UUID v4s, as hex.
#[must_use]
pub fn random_token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

/// What the database keeps of a session token.
#[must_use]
pub fn hash(token: &str) -> String {
    crate::auth::hex(&Sha256::digest(token.as_bytes()))
}

/// Starts a session for `user`; returns the `Set-Cookie` value.
///
/// # Errors
///
/// On database errors.
pub async fn start_session(
    state: &AppState,
    login: &Login,
    user: &db::User,
) -> anyhow::Result<String> {
    let token = random_token();
    db::create_session(
        &state.db,
        user.id,
        &hash(&token),
        db::now() + SESSION_DAYS * DAY,
    )
    .await?;
    Ok(session_cookie(login, &token))
}

fn session_cookie(login: &Login, token: &str) -> String {
    format!(
        "{COOKIE}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}{}",
        SESSION_DAYS * DAY,
        login.secure()
    )
}

/// Clears the session cookie.
#[must_use]
pub fn logout_cookie() -> String {
    format!("{COOKIE}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0")
}

/// Ends the session the request carries, if any.
///
/// # Errors
///
/// On database errors.
pub async fn end_session(state: &AppState, headers: &HeaderMap) -> anyhow::Result<()> {
    if let Some(token) = crate::auth::cookie(headers, COOKIE) {
        db::delete_session(&state.db, &hash(token)).await?;
    }
    Ok(())
}

/// Who is logged in, put in the request's extensions by [`load`].
#[derive(Debug, Clone, Default)]
pub struct CurrentUser(pub Option<db::User>);

/// Middleware: finds the session's user, and extends a session used in its second half.
pub async fn load(State(state): State<AppState>, mut request: Request, next: Next) -> Response {
    let mut renewed = None;
    let mut user = None;
    if let Some(login) = &state.login
        && let Some(token) = crate::auth::cookie(request.headers(), COOKIE)
    {
        let token_hash = hash(token);
        match db::session_user(&state.db, &token_hash).await {
            Ok(Some((found, expires))) => {
                let now = db::now();
                if expires - now < SESSION_DAYS * DAY / 2 {
                    match db::extend_session(&state.db, &token_hash, now + SESSION_DAYS * DAY).await
                    {
                        Ok(()) => renewed = Some(session_cookie(login, token)),
                        Err(e) => tracing::warn!("extending a session: {e:#}"),
                    }
                }
                user = Some(found);
            }
            Ok(None) => {}
            Err(e) => tracing::error!("looking up a session: {e:#}"),
        }
    }
    request.extensions_mut().insert(CurrentUser(user));
    let mut response = next.run(request).await;
    if let Some(cookie) = renewed.and_then(|c| HeaderValue::from_str(&c).ok()) {
        response.headers_mut().append(header::SET_COOKIE, cookie);
    }
    response
}
