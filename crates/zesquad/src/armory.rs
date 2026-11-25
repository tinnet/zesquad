//! The Battle.net API client: an app token from the client credentials, character lookups in
//! WoW: Forever's realmless form, and auction house dumps, in any region. See
//! `docs/battlenet-api.md`.

use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context as _, bail};
use reqwest::{StatusCode, Url, header};
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::Mutex;
use zesquad_core::Region;
use zesquad_core::armory::Endpoint;
use zesquad_core::auctions::RULESET;
use zesquad_core::guild::{self, Slug};
use zesquad_core::items::Source;
use zesquad_core::retry;

/// Battle.net's OAuth token endpoint, for every region.
pub const TOKEN_URL: &str = "https://oauth.battle.net/token";

/// For a character lookup or a token.
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(20);
/// For an auction dump: 20-30 MB of JSON (2-3 MB gzipped).
const DUMP_TIMEOUT: Duration = Duration::from_mins(2);

/// As whom to call Battle.net, and where when not the region's own host.
#[derive(Debug, Clone)]
pub struct Config {
    pub client_id: String,
    pub client_secret: String,
    /// Replaces `https://{region}.api.blizzard.com` for every region (tests point it at a fake).
    pub api_url: Option<Url>,
    pub token_url: Url,
}

impl Config {
    /// # Errors
    ///
    /// When the URLs don't parse.
    pub fn new(
        client_id: String,
        client_secret: String,
        api_url: Option<&str>,
        token_url: &str,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            api_url: api_url
                .map(|url| Url::parse(url).context("Battle.net API URL"))
                .transpose()?,
            token_url: Url::parse(token_url).context("Battle.net token URL")?,
            client_id,
            client_secret,
        })
    }

    fn api_url(&self, region: Region) -> anyhow::Result<Url> {
        match &self.api_url {
            Some(url) => Ok(url.clone()),
            None => Ok(Url::parse(&format!("https://{region}.api.blizzard.com"))?),
        }
    }
}

/// An auction dump, or that it hasn't changed since the time asked about.
pub enum Dump {
    Changed {
        body: String,
        /// Battle.net's `Last-Modified`, to send back as `If-Modified-Since` next time.
        last_modified: Option<String>,
    },
    NotModified,
}

/// A response: its status, body and `Last-Modified`.
struct Reply {
    status: StatusCode,
    body: String,
    last_modified: Option<String>,
}

/// What a lookup found.
pub enum Lookup {
    Found(Value),
    NotFound,
}

/// Battle.net answered 429 Too Many Requests: nothing is sent to it before `until`, the
/// time its `Retry-After` asked for.
#[derive(Debug)]
pub struct RateLimited {
    pub until: Instant,
}

impl std::fmt::Display for RateLimited {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let wait = self.until.saturating_duration_since(Instant::now());
        write!(
            f,
            "Battle.net asked us to slow down (429); waiting {}s",
            wait.as_secs().max(1)
        )
    }
}

impl std::error::Error for RateLimited {}

pub struct Client {
    http: reqwest::Client,
    config: Config,
    token: Mutex<Option<Token>>,
    /// After a 429, when Battle.net may be asked again. The quota is per client, so this
    /// holds every request back, not only the one that was refused.
    paused_until: std::sync::Mutex<Option<Instant>>,
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
            .timeout(LOOKUP_TIMEOUT)
            .build()?;
        Ok(Self {
            http,
            config,
            token: Mutex::new(None),
            paused_until: std::sync::Mutex::new(None),
        })
    }

    /// Looks up one endpoint of a character by its key (`name-surname`) in its region.
    ///
    /// # Errors
    ///
    /// On network errors, bad credentials, rate limiting or unexpected statuses.
    pub async fn character(
        &self,
        region: Region,
        key: &str,
        endpoint: Endpoint,
    ) -> anyhow::Result<Lookup> {
        let mut segments = vec!["profile", "wow", "character", key];
        segments.extend_from_slice(endpoint.path());
        self.lookup(
            region,
            &segments,
            &region.profile_namespace(),
            endpoint.as_str(),
        )
        .await
    }

    /// Looks up one document of a guild by its slug in its region.
    ///
    /// # Errors
    ///
    /// On network errors, bad credentials, rate limiting or unexpected statuses.
    pub async fn guild(
        &self,
        region: Region,
        slug: &Slug,
        endpoint: guild::Endpoint,
    ) -> anyhow::Result<Lookup> {
        let mut segments = vec!["data", "wow", "guild", slug.as_str()];
        segments.extend_from_slice(endpoint.path());
        self.lookup(
            region,
            &segments,
            &region.profile_namespace(),
            endpoint.as_str(),
        )
        .await
    }

    /// Looks up an item in one source's static item data.
    ///
    /// # Errors
    ///
    /// On network errors, bad credentials, rate limiting or unexpected statuses.
    pub async fn item(&self, region: Region, id: u32, source: Source) -> anyhow::Result<Lookup> {
        let id = id.to_string();
        self.lookup(
            region,
            &["data", "wow", "item", &id],
            &source.namespace(region),
            &format!("item {id}"),
        )
        .await
    }

    /// Looks up an item's media (its icon) in Forever's static data.
    ///
    /// # Errors
    ///
    /// On network errors, bad credentials, rate limiting or unexpected statuses.
    pub async fn item_media(&self, region: Region, id: u32) -> anyhow::Result<Lookup> {
        let id = id.to_string();
        self.lookup(
            region,
            &["data", "wow", "media", "item", &id],
            &Source::Forever.namespace(region),
            &format!("item {id}'s media"),
        )
        .await
    }

    /// A JSON document from a namespace: found, or a 404.
    async fn lookup(
        &self,
        region: Region,
        segments: &[&str],
        namespace: &str,
        what: &str,
    ) -> anyhow::Result<Lookup> {
        let Reply { status, body, .. } = self
            .get(region, segments, namespace, LOOKUP_TIMEOUT, None)
            .await?;
        match status {
            StatusCode::OK => Ok(Lookup::Found(
                serde_json::from_str(&body).context("Battle.net sent invalid JSON")?,
            )),
            StatusCode::NOT_FOUND => Ok(Lookup::NotFound),
            other => bail!("Battle.net answered {other} for {what}"),
        }
    }

    /// A house's auctions as JSON: its items, or with `commodities` its stackables. With
    /// `if_modified_since` (a `Last-Modified` from before), a dump that hasn't changed since
    /// isn't sent again.
    ///
    /// # Errors
    ///
    /// On network errors, bad credentials or statuses other than 200 and 304.
    pub async fn auctions(
        &self,
        region: Region,
        house: u32,
        commodities: bool,
        if_modified_since: Option<&str>,
    ) -> anyhow::Result<Dump> {
        let (ruleset, house_id) = (RULESET.to_string(), house.to_string());
        let mut segments = vec!["data", "wow", "ruleset", &ruleset, "auctions", &house_id];
        if commodities {
            segments.push("commodities");
        }
        let reply = self
            .get(
                region,
                &segments,
                &region.dynamic_namespace(),
                DUMP_TIMEOUT,
                if_modified_since,
            )
            .await?;
        match reply.status {
            StatusCode::OK => Ok(Dump::Changed {
                body: reply.body,
                last_modified: reply.last_modified,
            }),
            StatusCode::NOT_MODIFIED if if_modified_since.is_some() => Ok(Dump::NotModified),
            other => bail!("Battle.net answered {other} for auction house {house}"),
        }
    }

    /// GETs any API path and returns the status and body, for exploring the API.
    ///
    /// # Errors
    ///
    /// On network errors or when no token can be had.
    pub async fn raw(
        &self,
        region: Region,
        path: &str,
        namespace: &str,
    ) -> anyhow::Result<(StatusCode, String)> {
        let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        let reply = self
            .get(region, &segments, namespace, DUMP_TIMEOUT, None)
            .await?;
        Ok((reply.status, reply.body))
    }

    /// Fails with [`RateLimited`] while a 429's `Retry-After` hasn't passed.
    fn check_paused(&self) -> Result<(), RateLimited> {
        let mut paused = self
            .paused_until
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match *paused {
            Some(until) if until > Instant::now() => Err(RateLimited { until }),
            _ => {
                *paused = None;
                Ok(())
            }
        }
    }

    /// Holds requests back for as long as a 429's `Retry-After` asks.
    fn pause(&self, retry_after: Option<&str>) -> RateLimited {
        let until = Instant::now() + retry::retry_after(retry_after, SystemTime::now());
        let mut paused = self
            .paused_until
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let until = paused.map_or(until, |p| p.max(until));
        *paused = Some(until);
        RateLimited { until }
    }

    /// One retry with a fresh token on 401, in case Battle.net revoked the cached one early.
    /// Sends nothing while paused after a 429 (see [`RateLimited`]).
    async fn get(
        &self,
        region: Region,
        segments: &[&str],
        namespace: &str,
        timeout: Duration,
        if_modified_since: Option<&str>,
    ) -> anyhow::Result<Reply> {
        let mut url = self.config.api_url(region)?;
        url.path_segments_mut()
            .map_err(|()| anyhow::anyhow!("API URL can't have a path"))?
            .pop_if_empty()
            .extend(segments);
        url.query_pairs_mut()
            .append_pair("namespace", namespace)
            .append_pair("locale", region.locale());
        let mut retried = false;
        loop {
            self.check_paused()?;
            let token = self.token().await?;
            let mut request = self
                .http
                .get(url.clone())
                .bearer_auth(token)
                .timeout(timeout);
            if let Some(since) = if_modified_since {
                request = request.header(header::IF_MODIFIED_SINCE, since);
            }
            let response = request.send().await?;
            let status = response.status();
            if status == StatusCode::TOO_MANY_REQUESTS {
                let retry_after = response
                    .headers()
                    .get(header::RETRY_AFTER)
                    .and_then(|v| v.to_str().ok());
                let limited = self.pause(retry_after);
                tracing::warn!(
                    wait_secs = limited
                        .until
                        .saturating_duration_since(Instant::now())
                        .as_secs(),
                    "Battle.net answered 429; holding requests back"
                );
                return Err(limited.into());
            }
            if status != StatusCode::UNAUTHORIZED {
                let last_modified = response
                    .headers()
                    .get(header::LAST_MODIFIED)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_owned);
                return Ok(Reply {
                    status,
                    body: response.text().await?,
                    last_modified,
                });
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
