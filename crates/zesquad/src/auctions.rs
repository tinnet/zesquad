//! Keeps auction house prices: a snapshot of each followed house about hourly, condensed into
//! daily prices, and the details (names) of the items seen there. Runs beside the Armory sync,
//! so a slow dump never holds up a character.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use sqlx::SqlitePool;
use zesquad_core::auctions::{self, House};
use zesquad_core::items::{self, Source};

use crate::archive::Archive;
use crate::armory::{Client, Dump, Lookup, RateLimited};
use crate::db;

/// How often the loop looks for houses to check.
const TICK: Duration = Duration::from_mins(1);
/// How often a house is asked for. Battle.net updates the dumps about hourly at times of its
/// own, so a dump is fetched up to four times an hour; one that hasn't changed isn't stored.
const CHECK_EVERY: i64 = 15 * 60;
/// Whole days of snapshots kept; older days are only kept condensed.
const KEEP_DAYS: i64 = 7;
/// Items asked about per round, at most two requests each, and icons, one each: the first
/// 7,600 or so take about an hour, at most 21,600 requests, within Battle.net's 36,000 an hour.
const ITEMS_PER_ROUND: u32 = 120;
/// Icons asked for per round, one request each (see [`ITEMS_PER_ROUND`]).
const ICONS_PER_ROUND: u32 = 120;
/// How long before asking again about an item no item data had: Forever's grows with patches.
const ASK_MISSING_AGAIN: i64 = 7 * 24 * 60 * 60;
/// And about one whose ask failed.
const ASK_FAILED_AGAIN: i64 = 60 * 60;

pub struct AuctionSync {
    pub db: SqlitePool,
    pub client: Arc<Client>,
    /// Keeps the raw dumps, when set.
    pub archive: Option<Arc<Archive>>,
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
            if let Err(e) = self.learn_items().await {
                tracing::warn!("storing item details failed: {e:#}");
            }
            if let Err(e) = self.learn_icons().await {
                tracing::warn!("storing item icons failed: {e:#}");
            }
            if let Some(archive) = &self.archive {
                match archive.upload().await {
                    Ok(0) => {}
                    Ok(uploaded) => tracing::info!(uploaded, "auction dumps archived"),
                    Err(e) => tracing::warn!("archiving auction dumps failed: {e:#}"),
                }
            }
            tokio::time::sleep(TICK).await;
        }
    }

    /// Asks Battle.net about the items due (see [`db::items_due`]): Forever's item data first,
    /// then Classic's. Stops at the first failure, which is likely to hit the rest too. A 429
    /// isn't the item's fault, so it isn't stored: the round just stops, and the client sends
    /// nothing until the `Retry-After` has passed. Fails only on database errors.
    ///
    /// # Errors
    ///
    /// On database errors.
    pub async fn learn_items(&self) -> anyhow::Result<()> {
        let now = db::now();
        let due = db::items_due(
            &self.db,
            now - ASK_MISSING_AGAIN,
            now - ASK_FAILED_AGAIN,
            ITEMS_PER_ROUND,
        )
        .await?;
        let (mut found, mut missing) = (0, 0);
        for id in due {
            match self.item(id).await {
                Ok(Some((item, json, source))) => {
                    found += 1;
                    let fetch = db::ItemFetch::Found {
                        item: &item,
                        source,
                        json: &json,
                    };
                    db::store_item(&self.db, id, now, fetch).await?;
                }
                Ok(None) => {
                    missing += 1;
                    db::store_item(&self.db, id, now, db::ItemFetch::Missing).await?;
                }
                Err(e) if e.downcast_ref::<RateLimited>().is_some() => {
                    tracing::debug!(item = id, "item lookups paused: {e}");
                    break;
                }
                Err(e) => {
                    let error = format!("{e:#}");
                    tracing::warn!(item = id, "item lookup failed: {error}");
                    db::store_item(&self.db, id, now, db::ItemFetch::Failed(&error)).await?;
                    break;
                }
            }
        }
        if found + missing > 0 {
            tracing::info!(found, missing, "item details looked up");
        }
        Ok(())
    }

    /// Asks Battle.net for the icons due (see [`db::icons_due`]), like [`Self::learn_items`].
    ///
    /// # Errors
    ///
    /// On database errors.
    pub async fn learn_icons(&self) -> anyhow::Result<()> {
        let now = db::now();
        let due = db::icons_due(
            &self.db,
            now - ASK_MISSING_AGAIN,
            now - ASK_FAILED_AGAIN,
            ICONS_PER_ROUND,
        )
        .await?;
        let (mut found, mut missing) = (0, 0);
        for id in due {
            let fetch = match self.client.item_media(items::REGION, id).await {
                Ok(Lookup::Found(json)) => match items::icon(&json) {
                    Ok(Some(icon)) => Some(icon),
                    Ok(None) => None,
                    Err(e) => {
                        let error = format!("media in an unexpected shape: {e}");
                        db::store_icon(&self.db, id, now, db::IconFetch::Failed(&error)).await?;
                        continue;
                    }
                },
                Ok(Lookup::NotFound) => None,
                Err(e) if e.downcast_ref::<RateLimited>().is_some() => {
                    tracing::debug!(item = id, "icon lookups paused: {e}");
                    break;
                }
                Err(e) => {
                    let error = format!("{e:#}");
                    tracing::warn!(item = id, "icon lookup failed: {error}");
                    db::store_icon(&self.db, id, now, db::IconFetch::Failed(&error)).await?;
                    break;
                }
            };
            if let Some(icon) = fetch {
                found += 1;
                db::store_icon(&self.db, id, now, db::IconFetch::Found(icon)).await?;
            } else {
                missing += 1;
                db::store_icon(&self.db, id, now, db::IconFetch::Missing).await?;
            }
        }
        if found + missing > 0 {
            tracing::info!(found, missing, "item icons looked up");
        }
        Ok(())
    }

    /// An item, its document and where it was found; `None` when no item data has a usable
    /// one. A document we can't read (one without a name, say) counts as not there, so the next
    /// source is asked.
    async fn item(&self, id: u32) -> anyhow::Result<Option<(items::Item, String, Source)>> {
        for source in Source::ALL {
            if let Lookup::Found(json) = self.client.item(items::REGION, id, source).await? {
                let json = json.to_string();
                match items::parse(&json) {
                    Ok(item) => return Ok(Some((item, json, source))),
                    Err(e) => tracing::debug!(item = id, source = source.as_str(), "unusable: {e}"),
                }
            }
        }
        Ok(None)
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
        match self.fetch(house, &modified, now).await {
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
        now: i64,
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
        // Only dumps Battle.net says are new get archived, not one fetched again below.
        let new = (
            matches!(items, Dump::Changed { .. }),
            matches!(commodities, Dump::Changed { .. }),
        );
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
        let (items, commodities): (Arc<str>, Arc<str>) = (items.into(), commodities.into());
        // Before reading them, so a dump in a shape we don't understand is kept too.
        if let Some(archive) = &self.archive {
            for (is_new, commodities, body, modified) in [
                (new.0, false, &items, &items_modified),
                (new.1, true, &commodities, &commodities_modified),
            ] {
                if !is_new {
                    continue;
                }
                let saved = archive
                    .save(
                        region,
                        id,
                        commodities,
                        modified.as_deref(),
                        now,
                        body.clone(),
                    )
                    .await;
                if let Err(e) = saved {
                    tracing::warn!(%region, house = id, "keeping an auction dump failed: {e:#}");
                }
            }
        }
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
