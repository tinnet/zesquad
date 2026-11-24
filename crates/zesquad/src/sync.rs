//! Keeps squad members' and viewed guilds' Armory data fresh in the background.

use std::sync::Arc;
use std::time::Duration;

use sqlx::SqlitePool;
use tokio::sync::Notify;
use zesquad_core::Region;
use zesquad_core::armory::{Endpoint, Profile};
use zesquad_core::discord;
use zesquad_core::guild::{self, Slug};

use crate::armory::{Client, Lookup};
use crate::db::{self, Fetched};
use crate::discord::Discord;

/// Without a wake-up, look for due characters this often.
const TICK: Duration = Duration::from_secs(60);
/// Guilds nobody opened for this long stop being refreshed.
const GUILD_INTEREST: i64 = 7 * 86_400;
/// Pause between characters. With ~6 requests each this stays far below Battle.net's
/// 100 requests per second, and a squad of 40 still refreshes in a few seconds.
const PAUSE: Duration = Duration::from_millis(100);

pub struct Syncer {
    pub db: SqlitePool,
    pub client: Arc<Client>,
    pub interval: Duration,
    /// Notified when a character or guild is added or a refresh asked for, so it's fetched
    /// right away.
    pub wake: Arc<Notify>,
    /// Tells squads' Discord channels about level-ups.
    pub discord: Arc<Discord>,
}

impl Syncer {
    /// Runs until the task is dropped.
    pub async fn run(self) {
        loop {
            if let Err(e) = self.refresh_due().await {
                tracing::warn!("armory refresh failed: {e:#}");
            }
            if let Err(e) = self.refresh_guilds().await {
                tracing::warn!("guild refresh failed: {e:#}");
            }
            tokio::select! {
                () = self.wake.notified() => {}
                () = tokio::time::sleep(TICK) => {}
            }
        }
    }

    /// Fetches every character that's due. Fails only on database errors; Battle.net errors
    /// are stored per character instead.
    async fn refresh_due(&self) -> anyhow::Result<()> {
        let interval = i64::try_from(self.interval.as_secs()).unwrap_or(i64::MAX);
        let due = db::due(&self.db, db::now(), interval).await?;
        for (region, key) in &due {
            let before = db::armory_level(&self.db, region.as_str(), key).await?;
            let fetched = self.fetch(*region, key).await;
            if let Fetched::Failed(e) = &fetched {
                tracing::warn!(character = %key, %region, "armory fetch failed: {e}");
            }
            let summary = match &fetched {
                Fetched::Found(docs) => docs
                    .iter()
                    .find(|(e, _)| *e == Endpoint::Summary)
                    .map(|(_, doc)| doc),
                _ => None,
            };
            let after = summary.and_then(|doc| doc["level"].as_u64());
            let class = summary
                .and_then(|doc| doc["character_class"]["name"].as_str())
                .map(str::to_owned);
            db::store(&self.db, region.as_str(), key, fetched).await?;
            if let Some(level) = discord::level_up(before, after) {
                self.announce_level(*region, key, level, class.as_deref())
                    .await?;
            }
            tokio::time::sleep(PAUSE).await;
        }
        Ok(())
    }

    /// Fetches every guild that's due and was viewed lately.
    async fn refresh_guilds(&self) -> anyhow::Result<()> {
        let interval = i64::try_from(self.interval.as_secs()).unwrap_or(i64::MAX);
        let now = db::now();
        for (region, slug) in db::due_guilds(&self.db, now, interval, now - GUILD_INTEREST).await? {
            let fetched = self.fetch_guild(region, &slug).await;
            if let Fetched::Failed(e) = &fetched {
                tracing::warn!(guild = %slug, %region, "guild fetch failed: {e}");
            }
            db::store_guild(&self.db, region, &slug, fetched).await?;
            tokio::time::sleep(PAUSE).await;
        }
        Ok(())
    }

    /// The guild document decides whether the guild exists; a 404 on the roster leaves it out.
    async fn fetch_guild(&self, region: Region, slug: &Slug) -> Fetched<guild::Endpoint> {
        let mut docs = Vec::new();
        for endpoint in guild::Endpoint::ALL {
            match self.client.guild(region, slug, endpoint).await {
                Ok(Lookup::Found(doc)) => docs.push((endpoint, doc)),
                Ok(Lookup::NotFound) if endpoint == guild::Endpoint::Guild => {
                    return Fetched::NotFound;
                }
                Ok(Lookup::NotFound) => {}
                Err(e) => return Fetched::Failed(format!("{e:#}")),
            }
        }
        Fetched::Found(docs)
    }

    /// Tells every squad with this character and a Discord channel that it went up a level.
    async fn announce_level(
        &self,
        region: Region,
        key: &str,
        level: u64,
        class: Option<&str>,
    ) -> anyhow::Result<()> {
        for squad in db::discord_squads_with(&self.db, region, key).await? {
            let link = self.discord.squad_link(&squad.squad_id);
            let message = discord::levelled(
                &squad.character,
                level,
                class,
                &squad.squad_name,
                link.as_deref(),
            );
            self.discord.send(squad.webhook, message);
        }
        Ok(())
    }

    /// The summary decides whether the character exists; the other endpoints are extras, so a
    /// 404 on one of them is skipped rather than failing the character.
    async fn fetch(&self, region: Region, key: &str) -> Fetched {
        let summary = match self.client.character(region, key, Endpoint::Summary).await {
            Ok(Lookup::Found(doc)) => doc,
            Ok(Lookup::NotFound) => return Fetched::NotFound,
            Err(e) => return Fetched::Failed(format!("{e:#}")),
        };
        let mut docs = Vec::new();
        for endpoint in Endpoint::ALL {
            if endpoint == Endpoint::Summary || !Profile::wants(&summary, endpoint) {
                continue;
            }
            match self.client.character(region, key, endpoint).await {
                Ok(Lookup::Found(doc)) => docs.push((endpoint, doc)),
                Ok(Lookup::NotFound) => {}
                Err(e) => return Fetched::Failed(format!("{e:#}")),
            }
        }
        docs.push((Endpoint::Summary, summary));
        Fetched::Found(docs)
    }
}
