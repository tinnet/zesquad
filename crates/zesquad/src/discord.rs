//! Posts a squad's news to its Discord channel. Failures are logged and dropped: a missed
//! message isn't worth retrying, and a squad page never waits for Discord except when someone
//! sets a webhook up and wants to know it works.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use reqwest::Url;
use zesquad_core::discord::Webhook;

use crate::web::squad_path;

/// Discord's API host.
pub const API_URL: &str = "https://discord.com";

pub struct Config {
    /// Discord's API host; only tests change it.
    pub api_url: Url,
    /// Where zesquad is reached, for links to squads in messages; `None` leaves them out.
    pub public_url: Option<Url>,
}

pub struct Discord {
    http: reqwest::Client,
    config: Config,
}

impl Discord {
    /// # Errors
    ///
    /// When the HTTP client can't be built.
    pub fn new(config: Config) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .user_agent(concat!("zesquad/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self { http, config })
    }

    /// The squad's page, for linking to from a message.
    #[must_use]
    pub fn squad_link(&self, squad_id: &str) -> Option<String> {
        let base = self.config.public_url.as_ref()?;
        Some(format!(
            "{}{}",
            base.as_str().trim_end_matches('/'),
            squad_path(squad_id)
        ))
    }

    /// Posts `content` and waits for Discord to take it.
    ///
    /// # Errors
    ///
    /// When Discord can't be reached or refuses the message; a deleted webhook answers 404.
    pub async fn post(&self, hook: &Webhook, content: &str) -> anyhow::Result<()> {
        let path = hook.path();
        let sep = if path.contains('?') { '&' } else { '?' };
        let url = self
            .config
            .api_url
            .join(&format!("{path}{sep}wait=true"))
            .context("webhook URL")?;
        let body = serde_json::json!({
            "content": content,
            "username": "zesquad",
            // No @everyone, @here, role or user pings, whatever a name says.
            "allowed_mentions": { "parse": [] },
        });
        let response = self
            .http
            .post(url)
            .json(&body)
            .send()
            .await
            .with_context(|| format!("posting to Discord {hook}"))?;
        let status = response.status();
        anyhow::ensure!(status.is_success(), "Discord {hook} answered {status}");
        Ok(())
    }

    /// Posts in the background, logging a failure.
    pub fn send(self: &Arc<Self>, hook: Webhook, content: String) {
        let discord = self.clone();
        tokio::spawn(async move {
            if let Err(e) = discord.post(&hook, &content).await {
                tracing::warn!("discord: {e:#}");
            }
        });
    }
}
