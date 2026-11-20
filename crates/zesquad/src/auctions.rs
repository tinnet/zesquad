//! Keeps auction house prices: a snapshot of each followed house about hourly, condensed into
//! daily prices. Runs beside the Armory sync, so a slow dump never holds up a character.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use sqlx::SqlitePool;
use zesquad_core::auctions::{self, House};

use crate::armory::Client;
use crate::db;

/// How often the loop looks for houses to check.
const TICK: Duration = Duration::from_mins(1);
/// How often a house is asked for. Battle.net updates the dumps about hourly at times of its
/// own, so a dump is fetched up to four times an hour; one that hasn't changed isn't stored.
const CHECK_EVERY: i64 = 15 * 60;
/// Whole days of snapshots kept; older days are only kept condensed.
const KEEP_DAYS: i64 = 7;

pub struct AuctionSync {
    pub db: SqlitePool,
    pub client: Arc<Client>,
}

impl AuctionSync {
    /// Runs until the task is dropped.
    pub async fn run(self) {
        loop {
            for house in auctions::followed() {
                if let Err(e) = self.check(house).await {
                    tracing::warn!(region = %house.region, house = house.id, "auction house check failed: {e:#}");
                }
            }
            tokio::time::sleep(TICK).await;
        }
    }

    /// Fetches a house if it's due, then condenses and prunes its snapshots. Fails only on
    /// database errors; Battle.net errors are stored for the house instead.
    async fn check(&self, house: House) -> anyhow::Result<()> {
        let now = db::now();
        let checked = db::auctions_checked_at(&self.db, house.region, house.id).await?;
        if checked.is_none_or(|at| now - at >= CHECK_EVERY) {
            match self.fetch(house).await {
                Ok(snapshot) => {
                    let items = snapshot.items.len();
                    let stored =
                        db::store_auctions(&self.db, house.region, house.id, now, &snapshot)
                            .await?;
                    tracing::info!(region = %house.region, house = house.id, items, stored, "auction house checked");
                }
                Err(e) => {
                    tracing::warn!(region = %house.region, house = house.id, "auction house fetch failed: {e:#}");
                    db::auctions_failed(&self.db, house.region, house.id, now, &format!("{e:#}"))
                        .await?;
                }
            }
        }
        db::condense_auctions(&self.db, house.region, house.id, now, KEEP_DAYS).await
    }

    async fn fetch(&self, house: House) -> anyhow::Result<auctions::Snapshot> {
        let items = self.client.auctions(house.region, house.id, false).await?;
        let commodities = self.client.auctions(house.region, house.id, true).await?;
        // Reading 50 MB of JSON takes a while; keep it off the threads serving pages.
        tokio::task::spawn_blocking(move || auctions::snapshot(&[&items, &commodities]))
            .await?
            .context("Battle.net sent auctions in an unexpected shape")
    }
}
