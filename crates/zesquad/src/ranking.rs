//! The auction overview's rankings, kept in the database. Ranking a region reads its houses'
//! latest snapshots and two days of daily prices, about 90k rows and half a second on a small
//! server, but they only change when a snapshot is stored, a day condensed or item details
//! learned. So the auction sync ranks a region after its data changes ([`refresh`]) and the
//! page reads the result ([`load`]), ranking it itself only if the sync hasn't yet.
//!
//! The overview can be narrowed to a faction's or a ruleset's houses (a [`Scope`]). The data is
//! read once and every scope ranked from it, nine small rankings stored together.

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use zesquad_core::Region;
use zesquad_core::auctions::{self, Faction, House, ItemPrices, Scope};
use zesquad_core::market;
use zesquad_core::sales::Sales;

use crate::db;

/// The stored format and ranking rules. Bump it when either changes, so rankings stored by an
/// older version are ranked again instead of read.
pub const VERSION: i64 = 3;
/// Rows each overview list keeps.
pub const ROWS: usize = 8;
/// The pools whose factions' prices are compared; a gap's `pool` indexes this.
pub const POOLS: [auctions::Pool; 2] = [auctions::Pool::Pvp, auctions::Pool::Pve];

/// The rankings behind the overview of one region. A move's or bargain's `house` indexes
/// `houses`.
#[derive(Debug, Serialize, Deserialize)]
pub struct Ranked {
    /// The region's followed houses; not stored, as they're the code's (their ids are in the
    /// source a ranking is stored with).
    #[serde(skip)]
    pub houses: Vec<House>,
    /// The last finished day the moves are from.
    pub last_day: Option<i64>,
    pub rising: Vec<market::Move>,
    pub falling: Vec<market::Move>,
    pub gaps: Vec<market::Gap>,
    pub bargains: Vec<market::Bargain>,
    pub biggest: Vec<market::Market>,
    /// What sold for the most a day over the day before the latest snapshot.
    pub sold: Vec<market::Sold>,
}

impl Ranked {
    /// Every item ranked, for looking their names up.
    #[must_use]
    pub fn items(&self) -> Vec<u32> {
        self.rising
            .iter()
            .chain(&self.falling)
            .map(|m| m.item)
            .chain(self.gaps.iter().map(|g| g.item))
            .chain(self.bargains.iter().map(|b| b.item))
            .chain(self.biggest.iter().map(|m| m.item))
            .chain(self.sold.iter().map(|s| s.item))
            .collect()
    }

    /// A house's label and the class colouring it.
    #[must_use]
    pub fn house(&self, i: usize) -> (String, &'static str) {
        let house = self.houses[i];
        let class = match house.faction {
            Faction::Alliance => "faction-alliance",
            Faction::Horde => "faction-horde",
        };
        (house.label(), class)
    }
}

fn houses(region: Region) -> Vec<House> {
    auctions::followed()
        .filter(|h| h.region == region)
        .collect()
}

/// What `region`'s ranking is made from: each house with its latest snapshot and last
/// condensed day, and when vendor prices last changed. A ranking made from the same is
/// current. A few small reads by key.
async fn source(db: &SqlitePool, region: Region) -> anyhow::Result<String> {
    let mut houses_from = Vec::new();
    for house in houses(region) {
        let latest = db::latest_snapshot_at(db, region, house.id).await?;
        let condensed = db::condensed_through(db, region, house.id).await?;
        houses_from.push((house.id, latest, condensed));
    }
    let items = db::items_changed_at(db).await?;
    Ok(serde_json::to_string(&(houses_from, items))?)
}

/// What a region's rankings are made from, read once for every scope. Indexes follow `houses`.
struct Data {
    houses: Vec<House>,
    /// Each house's latest prices.
    latest: Vec<Vec<ItemPrices>>,
    /// Each house's moves on the region's last finished day; empty for a house behind.
    moves: Vec<Vec<market::Move>>,
    /// Each house's sales in the 24 hours up to the region's latest snapshot.
    sold: Vec<Sales>,
    /// Faction gaps, by pool (indexing [`POOLS`]).
    gaps: Vec<Vec<market::Gap>>,
    /// Each house's listings under the vendor price.
    bargains: Vec<Vec<market::Bargain>>,
    last_day: Option<i64>,
}

/// Reads what `region`'s rankings are made from: the houses' latest snapshots, two days of
/// daily prices and a day of sales. Sales are up to the latest snapshot, not up to now, so the
/// rankings only change with their [`source`].
async fn read(db: &SqlitePool, region: Region) -> anyhow::Result<Data> {
    let houses = houses(region);
    let vendor = db::vendor_prices(db).await?;
    let mut latest: Vec<Vec<ItemPrices>> = Vec::new();
    let mut days = Vec::new();
    let mut latest_at = None;
    for house in &houses {
        let prices = db::latest_auctions(db, region, house.id).await?;
        latest_at = latest_at.max(prices.as_ref().map(|&(at, _)| at));
        latest.push(prices.map_or_else(Vec::new, |(_, p)| p));
        days.push(db::condensed_through(db, region, house.id).await?);
    }
    let mut sold = Vec::new();
    for house in &houses {
        sold.push(match latest_at {
            Some(at) => db::sales_since(db, region, house.id, None, at - auctions::DAY).await?,
            None => Sales::default(),
        });
    }
    // Moves are from the region's last finished day; a house that's behind (its fetches failing,
    // say) is left out rather than mixing in an older day.
    let last_day = days.iter().copied().max().flatten();
    let mut moves = Vec::new();
    for (i, house) in houses.iter().enumerate() {
        moves.push(match last_day.filter(|&d| days[i] == Some(d)) {
            Some(day) => {
                let now = db::day_prices(db, region, house.id, day).await?;
                let before = db::day_prices(db, region, house.id, day - 1).await?;
                market::moves(i, &before, &now)
            }
            None => Vec::new(),
        });
    }
    let gaps = POOLS
        .into_iter()
        .enumerate()
        .map(|(p, pool)| {
            let side = |faction| {
                houses
                    .iter()
                    .position(|h| h.pool == pool && h.faction == faction)
            };
            match (side(Faction::Alliance), side(Faction::Horde)) {
                (Some(a), Some(h)) => market::gaps(p, &latest[a], &latest[h]),
                _ => Vec::new(),
            }
        })
        .collect();
    let bargains = latest
        .iter()
        .enumerate()
        .map(|(i, prices)| market::under_vendor(i, prices, |id| vendor.get(&id).copied()))
        .collect();
    Ok(Data {
        houses,
        latest,
        moves,
        sold,
        gaps,
        bargains,
        last_day,
    })
}

/// Ranks the houses in `scope`: the last day's movers, listings under the vendor price, the
/// biggest markets and what sold most. Faction gaps compare both factions, so only the
/// ruleset narrows them.
fn rank(data: &Data, scope: Scope) -> Ranked {
    let picked: Vec<usize> = (0..data.houses.len())
        .filter(|&i| scope.contains(data.houses[i]))
        .collect();
    let moves = picked.iter().flat_map(|&i| data.moves[i].clone()).collect();
    let (rising, falling) = market::rising_and_falling(moves, ROWS);
    let gaps = POOLS
        .into_iter()
        .zip(&data.gaps)
        .filter(|(pool, _)| scope.pool.is_none_or(|p| p == *pool))
        .flat_map(|(_, gaps)| gaps.iter().copied())
        .collect();
    let bargains = picked
        .iter()
        .flat_map(|&i| data.bargains[i].iter().copied())
        .collect();
    let latest: Vec<&[ItemPrices]> = picked.iter().map(|&i| data.latest[i].as_slice()).collect();
    let sold: Vec<Sales> = picked.iter().map(|&i| data.sold[i].clone()).collect();
    Ranked {
        houses: data.houses.clone(),
        last_day: data.last_day,
        rising,
        falling,
        gaps: market::top_gaps(gaps, ROWS),
        bargains: market::top_bargains(bargains, ROWS),
        biggest: market::biggest(&latest, ROWS),
        sold: market::most_sold(&sold, ROWS),
    }
}

/// `region`'s rankings, one per scope in [`Scope::all`]'s order, and whether they were ranked
/// now: the stored ones if they're current (made by this [`VERSION`] from the same
/// [`source`]), else ranked again and stored. Checking costs a few small reads; reading the
/// data is the half second, and ranking every scope from it a few milliseconds more.
async fn current(db: &SqlitePool, region: Region, now: i64) -> anyhow::Result<(Vec<Ranked>, bool)> {
    // Read before ranking: data stored meanwhile then makes the next check rank again.
    let source = source(db, region).await?;
    let stored = db::ranking(db, region)
        .await?
        .filter(|s| s.version == VERSION && s.source == source)
        .and_then(|s| match serde_json::from_str::<Vec<Ranked>>(&s.ranked) {
            Ok(ranked) if ranked.len() == Scope::all().len() => Some(ranked),
            Ok(_) => None,
            Err(e) => {
                tracing::warn!(%region, "stored auction ranking unreadable, ranking again: {e}");
                None
            }
        });
    if let Some(mut ranked) = stored {
        for r in &mut ranked {
            r.houses = houses(region);
        }
        return Ok((ranked, false));
    }
    let data = read(db, region).await?;
    let ranked: Vec<Ranked> = Scope::all().into_iter().map(|s| rank(&data, s)).collect();
    let ranking = db::StoredRanking {
        version: VERSION,
        source,
        ranked: serde_json::to_string(&ranked)?,
    };
    db::store_ranking(db, region, &ranking, now).await?;
    Ok((ranked, true))
}

/// Ranks `region` again and stores it if its data changed since it was last ranked; the
/// auction sync calls this after each round, so visitors find it ranked. Returns whether it
/// ranked.
///
/// # Errors
///
/// On database errors.
pub async fn refresh(db: &SqlitePool, region: Region, now: i64) -> anyhow::Result<bool> {
    Ok(current(db, region, now).await?.1)
}

/// `region`'s current ranking of the houses in `scope`, for the overview: usually the stored
/// one; ranked (and stored) on the spot when the data changed since, as when the sync hasn't
/// got to it yet or isn't running.
///
/// # Errors
///
/// On database errors.
pub async fn load(
    db: &SqlitePool,
    region: Region,
    scope: Scope,
    now: i64,
) -> anyhow::Result<Ranked> {
    let i = Scope::all().iter().position(|&s| s == scope).unwrap_or(0);
    let (ranked, _) = current(db, region, now).await?;
    ranked
        .into_iter()
        .nth(i)
        .ok_or_else(|| anyhow::anyhow!("no ranking for {scope:?}"))
}
