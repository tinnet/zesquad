//! The Battle.net API client: an app token from the client credentials, and character lookups
//! in WoW: Forever's realmless form. See `docs/battlenet-api.md`.

use std::time::{Duration, Instant};

use anyhow::{Context as _, bail};
use reqwest::{StatusCode, Url};
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::Mutex;
use zesquad_core::armory::Endpoint;

/// Battle.net's OAuth token endpoint, for every region.
pub const TOKEN_URL: &str = "https://oauth.battle.net/token";

/// Where and as whom to call Battle.net.
#[derive(Debug, Clone)]
pub struct Config {
    pub client_id: String,
    pub client_secret: String,
    /// `eu`, `us`, `tw`: picks the API host and namespaces.
    pub region: String,
    /// `https://{region}.api.blizzard.com` unless overridden (tests point it at a fake).
    pub api_url: Url,
    pub token_url: Url,
}

impl Config {
    /// # Errors
    ///
    /// When the overridden URLs don't parse.
    pub fn new(
        client_id: String,
        client_secret: String,
        region: String,
        api_url: Option<&str>,
        token_url: &str,
    ) -> anyhow::Result<Self> {
        let api_url = match api_url {
            Some(url) => url.to_owned(),
            None => format!("https://{region}.api.blizzard.com"),
        };
        Ok(Self {
            api_url: Url::parse(&api_url).context("Battle.net API URL")?,
            token_url: Url::parse(token_url).context("Battle.net token URL")?,
            client_id,
            client_secret,
            region,
        })
    }

    /// The namespace for Forever characters in this region.
    #[must_use]
    pub fn profile_namespace(&self) -> String {
        format!("profile-forever-{}", self.region)
    }

    /// Strings come back in this language: the region's English.
    fn locale(&self) -> &'static str {
        if self.region == "eu" {
            "en_GB"
        } else {
            "en_US"
        }
    }
}

/// What a lookup found.
pub enum Lookup {
    Found(Value),
    NotFound,
}

pub struct Client {
    http: reqwest::Client,
    config: Config,
    token: Mutex<Option<Token>>,
}

struct Token {
    value: String,
    /// When to stop using it and fetch a new one, a little before it expires.
    renew_at: Instant,
}

/// How long a token that expires in `expires_in` is used: until five minutes before it
/// expires, or halfway for a short-lived one, so it's always reused at least for a while.
fn use_for(expires_in: Duration) -> Duration {
    expires_in.saturating_sub((expires_in / 2).min(Duration::from_secs(300)))
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: u64,
}

impl Client {
    /// # Errors
    ///
    /// When the HTTP client can't be built (TLS setup).
    pub fn new(config: Config) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(format!("zesquad/{}", crate::version::short()))
            .timeout(Duration::from_secs(20))
            .build()?;
        Ok(Self {
            http,
            config,
            token: Mutex::new(None),
        })
    }

    /// Looks up one endpoint of a character by its key (`name-surname`).
    ///
    /// # Errors
    ///
    /// On network errors, bad credentials, rate limiting or unexpected statuses.
    pub async fn character(&self, key: &str, endpoint: Endpoint) -> anyhow::Result<Lookup> {
        let mut segments = vec!["profile", "wow", "character", key];
        segments.extend_from_slice(endpoint.path());
        let (status, body) = self
            .get(&segments, &self.config.profile_namespace())
            .await?;
        match status {
            StatusCode::OK => Ok(Lookup::Found(
                serde_json::from_str(&body).context("Battle.net sent invalid JSON")?,
            )),
            StatusCode::NOT_FOUND => Ok(Lookup::NotFound),
            other => bail!("Battle.net answered {other} for {}", endpoint.as_str()),
        }
    }

    /// GETs any API path and returns the status and body, for exploring the API.
    ///
    /// # Errors
    ///
    /// On network errors or when no token can be had.
    pub async fn raw(&self, path: &str, namespace: &str) -> anyhow::Result<(StatusCode, String)> {
        let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        self.get(&segments, namespace).await
    }

    /// One retry with a fresh token on 401, in case Battle.net revoked the cached one early.
    async fn get(
        &self,
        segments: &[&str],
        namespace: &str,
    ) -> anyhow::Result<(StatusCode, String)> {
        let mut url = self.config.api_url.clone();
        url.path_segments_mut()
            .map_err(|()| anyhow::anyhow!("API URL can't have a path"))?
            .pop_if_empty()
            .extend(segments);
        url.query_pairs_mut()
            .append_pair("namespace", namespace)
            .append_pair("locale", self.config.locale());
        let mut retried = false;
        loop {
            let token = self.token().await?;
            let response = self.http.get(url.clone()).bearer_auth(token).send().await?;
            let status = response.status();
            if status != StatusCode::UNAUTHORIZED {
                return Ok((status, response.text().await?));
            }
            if retried {
                bail!("Battle.net rejected a fresh token (401); check the client id and secret");
            }
            *self.token.lock().await = None;
            retried = true;
        }
    }

    /// The cached app token, fetching a new one when it's close to expiring (see [`use_for`]).
    async fn token(&self) -> anyhow::Result<String> {
        let mut cached = self.token.lock().await;
        if let Some(token) = cached.as_ref()
            && token.renew_at > Instant::now()
        {
            return Ok(token.value.clone());
        }
        let response = self
            .http
            .post(self.config.token_url.clone())
            .basic_auth(&self.config.client_id, Some(&self.config.client_secret))
            .form(&[("grant_type", "client_credentials")])
            .send()
            .await
            .context("asking Battle.net for a token")?;
        if !response.status().is_success() {
            bail!(
                "Battle.net refused the client credentials ({}); check ZESQUAD_BNET_CLIENT_ID and _SECRET",
                response.status()
            );
        }
        let token: TokenResponse = response.json().await.context("reading the token")?;
        let value = token.access_token;
        *cached = Some(Token {
            value: value.clone(),
            renew_at: Instant::now() + use_for(Duration::from_secs(token.expires_in)),
        });
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_renewed_before_they_expire_but_always_reused() {
        let secs = Duration::from_secs;
        assert_eq!(use_for(secs(86_399)), secs(86_099), "five minutes early");
        assert_eq!(use_for(secs(600)), secs(300));
        assert_eq!(use_for(secs(300)), secs(150), "short-lived: half its life");
        assert_eq!(use_for(secs(0)), secs(0));
    }
}
