//! Keeps squad members' Armory data fresh in the background.

use std::sync::Arc;
use std::time::Duration;

use sqlx::SqlitePool;
use tokio::sync::Notify;
use zesquad_core::armory::{Endpoint, Profile};

use crate::armory::{Client, Lookup};
use crate::db::{self, Fetched};

/// Without a wake-up, look for due characters this often.
const TICK: Duration = Duration::from_secs(60);
/// Pause between characters. With ~6 requests each this stays far below Battle.net's
/// 100 requests per second, and a squad of 40 still refreshes in a few seconds.
const PAUSE: Duration = Duration::from_millis(100);

pub struct Syncer {
    pub db: SqlitePool,
    pub client: Arc<Client>,
    pub interval: Duration,
    /// Notified when a character is added or a refresh asked for, so it's fetched right away.
    pub wake: Arc<Notify>,
}

impl Syncer {
    /// Runs until the task is dropped.
    pub async fn run(self) {
        loop {
            if let Err(e) = self.refresh_due().await {
                tracing::warn!("armory refresh failed: {e:#}");
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
            let fetched = self.fetch(key).await;
            if let Fetched::Failed(e) = &fetched {
                tracing::warn!(character = %key, "armory fetch failed: {e}");
            }
            db::store(&self.db, region, key, fetched).await?;
            tokio::time::sleep(PAUSE).await;
        }
        Ok(())
    }

    /// The summary decides whether the character exists; the other endpoints are extras, so a
    /// 404 on one of them is skipped rather than failing the character.
    async fn fetch(&self, key: &str) -> Fetched {
        let summary = match self.client.character(key, Endpoint::Summary).await {
            Ok(Lookup::Found(doc)) => doc,
            Ok(Lookup::NotFound) => return Fetched::NotFound,
            Err(e) => return Fetched::Failed(format!("{e:#}")),
        };
        let mut docs = Vec::new();
        for endpoint in Endpoint::ALL {
            if endpoint == Endpoint::Summary || !Profile::wants(&summary, endpoint) {
                continue;
            }
            match self.client.character(key, endpoint).await {
                Ok(Lookup::Found(doc)) => docs.push((endpoint, doc)),
                Ok(Lookup::NotFound) => {}
                Err(e) => return Fetched::Failed(format!("{e:#}")),
            }
        }
        docs.push((Endpoint::Summary, summary));
        Fetched::Found(docs)
    }
}
