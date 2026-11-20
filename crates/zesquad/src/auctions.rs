//! Keeps auction house prices: a snapshot of each followed house about hourly, condensed into
//! daily prices. Runs beside the Armory sync, so a slow dump never holds up a character.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use sqlx::SqlitePool;
use zesquad_core::auctions::{self, House};

use crate::armory::{Client, Dump};
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
            self.update(house, now).await?;
        }
        db::condense_auctions(&self.db, house.region, house.id, now, KEEP_DAYS).await
    }

    /// Fetches a house now and stores a snapshot if its dumps changed. Fails only on database
    /// errors; Battle.net errors are stored for the house instead.
    ///
    /// # Errors
    ///
    /// On database errors.
    pub async fn update(&self, house: House, now: i64) -> anyhow::Result<()> {
        let modified = db::auctions_modified(&self.db, house.region, house.id).await?;
        match self.fetch(house, &modified).await {
            Ok(None) => {
                tracing::debug!(region = %house.region, house = house.id, "auction house not modified");
                db::auctions_unchanged(&self.db, house.region, house.id, now).await?;
            }
            Ok(Some((snapshot, modified))) => {
                let items = snapshot.items.len();
                let stored =
                    db::store_auctions(&self.db, house.region, house.id, now, &snapshot).await?;
                db::set_auctions_modified(&self.db, house.region, house.id, &modified).await?;
                tracing::info!(region = %house.region, house = house.id, items, stored, "auction house checked");
            }
            Err(e) => {
                tracing::warn!(region = %house.region, house = house.id, "auction house fetch failed: {e:#}");
                db::auctions_failed(&self.db, house.region, house.id, now, &format!("{e:#}"))
                    .await?;
            }
        }
        Ok(())
    }

    /// Both dumps of a house read into a snapshot, with their `Last-Modified`; `None` when
    /// neither changed since `modified`. Each is asked for with `If-Modified-Since`, which
    /// Battle.net answers with a 304 and no body when the dump hasn't changed: polite, and
    /// saves downloading megabytes three checks out of four.
    async fn fetch(
        &self,
        house: House,
        modified: &db::DumpsModified,
    ) -> anyhow::Result<Option<(auctions::Snapshot, db::DumpsModified)>> {
        let (region, id) = (house.region, house.id);
        let items = self
            .client
            .auctions(region, id, false, modified.items.as_deref())
            .await?;
        let commodities = self
            .client
            .auctions(region, id, true, modified.commodities.as_deref())
            .await?;
        let (items, commodities) = match (items, commodities) {
            (Dump::NotModified, Dump::NotModified) => return Ok(None),
            // A snapshot needs both: ask again, unconditionally, for the one that didn't change.
            (items, Dump::NotModified) => {
                (items, self.client.auctions(region, id, true, None).await?)
            }
            (Dump::NotModified, commodities) => (
                self.client.auctions(region, id, false, None).await?,
                commodities,
            ),
            both => both,
        };
        let (
            Dump::Changed {
                body: items,
                last_modified: items_modified,
            },
            Dump::Changed {
                body: commodities,
                last_modified: commodities_modified,
            },
        ) = (items, commodities)
        else {
            anyhow::bail!("Battle.net answered 304 to a request without If-Modified-Since");
        };
        // Reading 50 MB of JSON takes a while; keep it off the threads serving pages.
        let snapshot =
            tokio::task::spawn_blocking(move || auctions::snapshot(&[&items, &commodities]))
                .await?
                .context("Battle.net sent auctions in an unexpected shape")?;
        Ok(Some((
            snapshot,
            db::DumpsModified {
                items: items_modified,
                commodities: commodities_modified,
            },
        )))
    }
}
