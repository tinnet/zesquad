//! Command-line arguments. Every option can also be set by its `ZESQUAD_*` environment
//! variable (or a `.env` file), which is how containers configure it.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context as _;
use clap::{Args, Parser, Subcommand};

use zesquad_core::Region;

use crate::version;
use crate::{archive, armory, s3};

#[derive(Debug, Parser)]
#[command(
    name = "zesquad",
    about = "Friend squads for WoW: Forever, with characters looked up on the Battle.net Armory",
    version = version::short(),
    long_version = version::long(),
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Serve the web app. Creates and writes the database.
    /// Boxed: its many options would make every other command as large.
    Serve(Box<ServeArgs>),
    /// Ask a running zesquad's /healthz; exits 0 when healthy. Read-only (for container health checks).
    Healthcheck {
        /// Address zesquad listens on.
        #[arg(long, env = "ZESQUAD_LISTEN", default_value = "127.0.0.1:8080")]
        listen: SocketAddr,
    },
    /// GET a Battle.net API path with the app token and print the status and body. Read-only.
    Api {
        /// Path on the API host, e.g. /data/wow/region/index.
        path: String,
        /// Battle.net namespace, e.g. dynamic-forever-eu.
        #[arg(long, default_value = "profile-forever-eu")]
        namespace: String,
        #[command(flatten)]
        bnet: BnetArgs,
    },
}

#[derive(Debug, Args)]
pub struct ServeArgs {
    /// Address to listen on.
    #[arg(long, env = "ZESQUAD_LISTEN", default_value = "127.0.0.1:8080")]
    pub listen: SocketAddr,
    /// SQLite database file; created if missing.
    #[arg(long, env = "ZESQUAD_DATABASE", default_value = "data/zesquad.db")]
    pub database: PathBuf,
    /// Minutes between Armory refreshes of a character.
    #[arg(long, env = "ZESQUAD_REFRESH_MINUTES", default_value_t = 60, value_parser = clap::value_parser!(u64).range(1..))]
    pub refresh_minutes: u64,
    /// Password everyone must enter before seeing any squad. Empty or unset: no password.
    #[arg(long, env = "ZESQUAD_PASSWORD", hide_env_values = true)]
    pub password: Option<String>,
    /// Follow auction house prices (EU and US, PVE and PVP, both factions): a snapshot about
    /// hourly, kept a week, and daily prices kept for good. Needs the Battle.net credentials.
    #[arg(long, env = "ZESQUAD_AUCTIONS")]
    pub auctions: bool,
    #[command(flatten)]
    pub archive: ArchiveArgs,
    /// Log JSON lines instead of text.
    #[arg(long, env = "ZESQUAD_LOG_JSON")]
    pub log_json: bool,
    /// Where people reach zesquad, e.g. `https://zesquad.example`. With the Battle.net
    /// credentials it turns on logging in with Battle.net; `<url>/auth/callback` must be a
    /// redirect URL of the Battle.net API client.
    #[arg(long, env = "ZESQUAD_PUBLIC_URL")]
    pub public_url: Option<String>,
    /// Behind a reverse proxy, where it puts each client's address, for the rate limits.
    /// Without a proxy, or with the wrong one, clients could claim any address.
    #[arg(long, env = "ZESQUAD_TRUST_PROXY", value_enum, default_value_t)]
    pub trust_proxy: crate::limits::TrustProxy,
    /// Discord's API host, which squads' webhooks post to; only tests change it.
    #[arg(
        long,
        env = "ZESQUAD_DISCORD_API_URL",
        hide = true,
        default_value = crate::discord::API_URL
    )]
    pub discord_api_url: String,
    #[command(flatten)]
    pub bnet: BnetArgs,
}

impl ServeArgs {
    /// Posting to Discord; messages link to squads when the public URL is set.
    ///
    /// # Errors
    ///
    /// When a URL doesn't parse.
    pub fn discord(&self) -> anyhow::Result<crate::discord::Config> {
        let public_url = self
            .public_url
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(reqwest::Url::parse)
            .transpose()
            .context("ZESQUAD_PUBLIC_URL")?;
        Ok(crate::discord::Config {
            api_url: reqwest::Url::parse(&self.discord_api_url)
                .context("ZESQUAD_DISCORD_API_URL")?,
            public_url,
        })
    }

    /// Logging in, when both the public URL and the Battle.net credentials are set.
    ///
    /// # Errors
    ///
    /// When a URL doesn't parse.
    pub fn login(&self) -> anyhow::Result<Option<crate::login::Config>> {
        let Some(public_url) = self.public_url.as_deref().filter(|s| !s.is_empty()) else {
            return Ok(None);
        };
        let Some(bnet) = self.bnet.config()? else {
            return Ok(None);
        };
        let public_url = reqwest::Url::parse(public_url).context("ZESQUAD_PUBLIC_URL")?;
        let oauth_url =
            reqwest::Url::parse(&format!("{}/", self.bnet.oauth_url.trim_end_matches('/')))
                .context("Battle.net OAuth URL")?;
        Ok(Some(crate::login::Config {
            client_id: bnet.client_id,
            client_secret: bnet.client_secret,
            oauth_url,
            token_url: bnet.token_url,
            public_url,
        }))
    }

    #[must_use]
    pub fn refresh(&self) -> Duration {
        Duration::from_secs(self.refresh_minutes.saturating_mul(60))
    }
}

#[derive(Debug, Args)]
pub struct ArchiveArgs {
    /// Keep every auction dump, compressed, in this folder. With a bucket, they're uploaded
    /// there and the folder only holds what's waiting. Unset: dumps aren't kept.
    #[arg(long, env = "ZESQUAD_ARCHIVE_DIR")]
    pub archive_dir: Option<PathBuf>,
    /// S3-compatible bucket for the dumps, path-style: the endpoint's URL, then /BUCKET (and
    /// optionally /PREFIX for the keys).
    #[arg(long, env = "ZESQUAD_ARCHIVE_S3_URL", requires_all = ["archive_dir", "archive_s3_access_key_id", "archive_s3_secret_access_key"])]
    pub archive_s3_url: Option<String>,
    /// The bucket's signing region: auto for R2, the location (e.g. fsn1) for Hetzner.
    #[arg(long, env = "ZESQUAD_ARCHIVE_S3_REGION", default_value = "auto")]
    pub archive_s3_region: String,
    /// Access key id for the bucket.
    #[arg(long, env = "ZESQUAD_ARCHIVE_S3_ACCESS_KEY_ID", hide_env_values = true)]
    pub archive_s3_access_key_id: Option<String>,
    /// Secret access key for the bucket.
    #[arg(
        long,
        env = "ZESQUAD_ARCHIVE_S3_SECRET_ACCESS_KEY",
        hide_env_values = true
    )]
    pub archive_s3_secret_access_key: Option<String>,
}

impl ArchiveArgs {
    /// `None` when no folder is set.
    ///
    /// # Errors
    ///
    /// When the bucket URL doesn't parse.
    pub fn config(&self) -> anyhow::Result<Option<archive::Config>> {
        let Some(dir) = self.archive_dir.clone() else {
            return Ok(None);
        };
        let bucket = match (
            &self.archive_s3_url,
            &self.archive_s3_access_key_id,
            &self.archive_s3_secret_access_key,
        ) {
            (Some(url), Some(id), Some(secret)) => Some(s3::Config {
                url: url.parse().context("ZESQUAD_ARCHIVE_S3_URL")?,
                region: self.archive_s3_region.clone(),
                access_key_id: id.clone(),
                secret_access_key: secret.clone(),
            }),
            _ => None,
        };
        Ok(Some(archive::Config { dir, bucket }))
    }
}

#[derive(Debug, Args)]
pub struct BnetArgs {
    /// Battle.net API client id. Without id and secret, Armory lookups are off.
    #[arg(long, env = "ZESQUAD_BNET_CLIENT_ID", hide_env_values = true)]
    pub client_id: Option<String>,
    /// Battle.net API client secret.
    #[arg(long, env = "ZESQUAD_BNET_CLIENT_SECRET", hide_env_values = true)]
    pub client_secret: Option<String>,
    /// Region new squads start in (eu, us or tw); `api` calls this region's host.
    #[arg(long, env = "ZESQUAD_BNET_REGION", default_value = "eu")]
    pub region: Region,
    /// API base URL; only tests change it.
    #[arg(long, env = "ZESQUAD_BNET_API_URL", hide = true)]
    pub api_url: Option<String>,
    /// OAuth token URL; only tests change it.
    #[arg(
        long,
        env = "ZESQUAD_BNET_TOKEN_URL",
        hide = true,
        default_value = crate::armory::TOKEN_URL
    )]
    pub token_url: String,
    /// OAuth host for logging in (`/authorize`, `/userinfo`); only tests change it.
    #[arg(
        long,
        env = "ZESQUAD_BNET_OAUTH_URL",
        hide = true,
        default_value = crate::login::OAUTH_URL
    )]
    pub oauth_url: String,
}

impl BnetArgs {
    /// `None` when the credentials aren't set.
    ///
    /// # Errors
    ///
    /// When an overridden URL doesn't parse.
    pub fn config(&self) -> anyhow::Result<Option<armory::Config>> {
        let (Some(id), Some(secret)) = (
            self.client_id.as_deref().filter(|s| !s.is_empty()),
            self.client_secret.as_deref().filter(|s| !s.is_empty()),
        ) else {
            return Ok(None);
        };
        armory::Config::new(
            id.to_owned(),
            secret.to_owned(),
            self.api_url.as_deref(),
            &self.token_url,
        )
        .map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory as _;

    #[test]
    fn parser_is_consistent() {
        Cli::command().debug_assert();
    }
}
