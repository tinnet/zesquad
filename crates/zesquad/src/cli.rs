//! Command-line arguments. Every option can also be set by its `ZESQUAD_*` environment
//! variable (or a `.env` file), which is how containers configure it.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use clap::{Args, Parser, Subcommand};

use crate::armory;
use crate::version;

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
    Serve(ServeArgs),
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
    /// Log JSON lines instead of text.
    #[arg(long, env = "ZESQUAD_LOG_JSON")]
    pub log_json: bool,
    #[command(flatten)]
    pub bnet: BnetArgs,
}

impl ServeArgs {
    #[must_use]
    pub fn refresh(&self) -> Duration {
        Duration::from_secs(self.refresh_minutes.saturating_mul(60))
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
    /// Battle.net region of the characters.
    #[arg(long, env = "ZESQUAD_BNET_REGION", default_value = "eu", value_parser = ["eu", "us", "tw"])]
    pub region: String,
    /// API base URL; only tests change it.
    #[arg(long, env = "ZESQUAD_BNET_API_URL", hide = true)]
    pub api_url: Option<String>,
    /// OAuth token URL; only tests change it.
    #[arg(
        long,
        env = "ZESQUAD_BNET_TOKEN_URL",
        hide = true,
        default_value = "https://oauth.battle.net/token"
    )]
    pub token_url: String,
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
            self.region.clone(),
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
