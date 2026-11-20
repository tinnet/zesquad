//! Auction house prices: which houses zesquad follows, and turning Battle.net's auction dumps
//! into prices per item. See "Auction houses" in `docs/battlenet-api.md`.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::Region;

/// Every ruleset lists the same houses with the same auctions, so any one does; this is Normal.
pub const RULESET: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Faction {
    Alliance,
    Horde,
}

impl Faction {
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Alliance => "Alliance",
            Self::Horde => "Horde",
        }
    }
}

/// Which servers a house serves. The API doesn't say; see [`HOUSES`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pool {
    Pvp,
    Pve,
}

impl Pool {
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Pvp => "PvP",
            Self::Pve => "PvE",
        }
    }
}

/// An auction house zesquad follows. House ids are the same in every region.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct House {
    pub region: Region,
    pub id: u32,
    pub faction: Faction,
    pub pool: Pool,
}

impl House {
    /// For people: `Alliance PvP`.
    #[must_use]
    pub fn label(self) -> String {
        format!("{} {}", self.faction.name(), self.pool.name())
    }
}

/// The regions whose houses are followed.
pub const REGIONS: [Region; 2] = [Region::Eu, Region::Us];

/// The houses followed in each region: the four big ones, about 100-150k auctions each. Which
/// of each faction's pair is PVP and which PVE is a community guess (Goldbook), not API data.
///
/// TODO: the other houses. 119/120 and 123/124 (Alliance/Horde, 12-25k auctions each) are
/// probably Roleplay and Roleplay PVP; 116, 117, 121 and 125 are Blackwater (neutral); 112,
/// 115 and 118 are empty.
pub const HOUSES: [(u32, Faction, Pool); 4] = [
    (110, Faction::Alliance, Pool::Pvp),
    (111, Faction::Alliance, Pool::Pve),
    (113, Faction::Horde, Pool::Pvp),
    (114, Faction::Horde, Pool::Pve),
];

/// Every house followed, region by region.
pub fn followed() -> impl Iterator<Item = House> {
    REGIONS.into_iter().flat_map(|region| {
        HOUSES.into_iter().map(move |(id, faction, pool)| House {
            region,
            id,
            faction,
            pool,
        })
    })
}

/// Leather, from the lightest; item ids as in Classic. Item names aren't fetched yet, so they're
/// written here.
pub const LEATHER: [(u32, &str); 5] = [
    (2318, "Light Leather"),
    (2319, "Medium Leather"),
    (4234, "Heavy Leather"),
    (4304, "Thick Leather"),
    (8170, "Rugged Leather"),
];

/// An amount of copper split into the coins the game shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Money {
    pub gold: u64,
    pub silver: u64,
    pub copper: u64,
    /// Copper is left out next to gold, see [`Money::brief`].
    pub brief: bool,
}

impl Money {
    #[must_use]
    pub fn from_copper(copper: u64) -> Self {
        Self {
            gold: copper / 10_000,
            silver: copper / 100 % 100,
            copper: copper % 100,
            brief: false,
        }
    }

    /// For small print: from a gold up, rounded to the silver and shown without copper.
    #[must_use]
    pub fn brief(copper: u64) -> Self {
        if copper < 10_000 {
            return Self::from_copper(copper);
        }
        Self {
            brief: true,
            ..Self::from_copper((copper + 50) / 100 * 100)
        }
    }

    /// Whether copper is shown: always, except next to gold in brief.
    #[must_use]
    pub fn shows_copper(self) -> bool {
        !(self.brief && self.gold > 0)
    }

    /// Whether gold is shown: the game leaves out empty larger units.
    #[must_use]
    pub fn shows_gold(self) -> bool {
        self.gold > 0
    }

    /// Whether silver is shown: when there's any, or any gold.
    #[must_use]
    pub fn shows_silver(self) -> bool {
        self.gold > 0 || self.silver > 0
    }
}

/// Copper as the game shows it: `1g 2s 3c`, leaving out empty larger units (`45c`, `2s 0c`).
#[must_use]
pub fn money(copper: u64) -> String {
    let m = Money::from_copper(copper);
    if m.shows_gold() {
        format!("{}g {}s {}c", m.gold, m.silver, m.copper)
    } else if m.shows_silver() {
        format!("{}s {}c", m.silver, m.copper)
    } else {
        format!("{}c", m.copper)
    }
}

/// One item's prices in one snapshot of a house, in copper per unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemPrices {
    pub item: u32,
    pub min_price: u64,
    /// Half the units on sale cost this much or less.
    pub median_price: u64,
    /// Units on sale.
    pub quantity: u64,
    pub auctions: u64,
}

/// A house's snapshot: prices per item, sorted by item id, and a fingerprint of the auctions in
/// it, equal for two dumps with the same auctions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub items: Vec<ItemPrices>,
    pub fingerprint: u64,
}

/// A house's dumps (`/auctions/{house}` with items, `/commodities` with stackables) share the
/// shape; items have a `buyout` for the whole auction, commodities a `unit_price`.
#[derive(Deserialize)]
struct Dump {
    #[serde(default)]
    auctions: Vec<Auction>,
}

#[derive(Deserialize)]
struct Auction {
    id: u64,
    item: ItemRef,
    quantity: u64,
    buyout: Option<u64>,
    unit_price: Option<u64>,
}

#[derive(Deserialize)]
struct ItemRef {
    id: u32,
}

impl Auction {
    /// `None` for bid-only auctions, which have no price to compare.
    fn unit_price(&self) -> Option<u64> {
        self.unit_price
            .or_else(|| self.buyout?.checked_div(self.quantity))
    }
}

/// Reads a house's dumps into one snapshot. Item variants (random suffixes, in `modifiers`)
/// count as the plain item.
///
/// # Errors
///
/// When a dump isn't the JSON expected.
pub fn snapshot(dumps: &[&str]) -> Result<Snapshot, serde_json::Error> {
    let mut by_item: BTreeMap<u32, Vec<(u64, u64)>> = BTreeMap::new();
    let mut fingerprint: u64 = 0;
    for dump in dumps {
        let dump: Dump = serde_json::from_str(dump)?;
        for auction in dump.auctions {
            // Order-independent: a sum of the ids, spread out so near ids don't cancel.
            fingerprint = fingerprint.wrapping_add(auction.id.wrapping_mul(0x9E37_79B9_7F4A_7C15));
            fingerprint = fingerprint.wrapping_add(1);
            if let Some(price) = auction.unit_price()
                && auction.quantity > 0
            {
                by_item
                    .entry(auction.item.id)
                    .or_default()
                    .push((price, auction.quantity));
            }
        }
    }
    let items = by_item
        .into_iter()
        .map(|(item, mut offers)| {
            offers.sort_unstable();
            ItemPrices {
                item,
                min_price: offers[0].0,
                median_price: weighted_median(&offers),
                quantity: offers.iter().map(|&(_, q)| q).sum(),
                auctions: offers.len() as u64,
            }
        })
        .collect();
    Ok(Snapshot { items, fingerprint })
}

/// The lower median of `(value, weight)` pairs sorted by value, each value counted `weight`
/// times. `sorted` isn't empty.
fn weighted_median(sorted: &[(u64, u64)]) -> u64 {
    let total: u64 = sorted.iter().map(|&(_, w)| w).sum();
    let half = total.div_ceil(2);
    let mut seen = 0;
    for &(value, weight) in sorted {
        seen += weight;
        if seen >= half {
            return value;
        }
    }
    sorted[sorted.len() - 1].0
}

/// One item's prices over a day, from that day's snapshots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DailyPrices {
    pub item: u32,
    /// The lowest price seen.
    pub min_price: u64,
    /// The median of the snapshots' medians: a typical price that day.
    pub median_price: u64,
    /// The highest of the snapshots' medians.
    pub max_price: u64,
    /// Units on sale, on average over the snapshots that had the item.
    pub quantity: u64,
    /// How many snapshots had the item.
    pub snapshots: u64,
}

/// Condenses one item's snapshots from one day. `None` without any.
#[must_use]
pub fn daily(item: u32, snapshots: &[ItemPrices]) -> Option<DailyPrices> {
    let count = snapshots.len() as u64;
    let mut medians: Vec<u64> = snapshots.iter().map(|s| s.median_price).collect();
    medians.sort_unstable();
    Some(DailyPrices {
        item,
        min_price: snapshots.iter().map(|s| s.min_price).min()?,
        median_price: medians[(medians.len() - 1) / 2],
        max_price: *medians.last()?,
        quantity: snapshots.iter().map(|s| s.quantity).sum::<u64>() / count,
        snapshots: count,
    })
}

/// The UTC day (days since the Unix epoch) a time in seconds falls on.
#[must_use]
pub fn day(at: i64) -> i64 {
    at.div_euclid(DAY)
}

/// Seconds in a day.
pub const DAY: i64 = 86_400;

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(format!(
            "{}/fixtures/{name}.json",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    }

    fn prices(item: u32, min: u64, median: u64, quantity: u64, auctions: u64) -> ItemPrices {
        ItemPrices {
            item,
            min_price: min,
            median_price: median,
            quantity,
            auctions,
        }
    }

    #[test]
    fn follows_pve_and_pvp_of_both_factions_in_eu_and_us() {
        let houses: Vec<_> = followed().map(|h| (h.region, h.id)).collect();
        assert_eq!(houses.len(), 8);
        for region in [Region::Eu, Region::Us] {
            for id in [110, 111, 113, 114] {
                assert!(houses.contains(&(region, id)));
            }
        }
    }

    #[test]
    fn items_and_commodities_make_one_snapshot() {
        let (items, commodities) = (fixture("auctions"), fixture("commodities"));
        let snapshot = snapshot(&[&items, &commodities]).unwrap();
        assert_eq!(
            snapshot.items,
            [
                // Commodities: a median by units, not by auctions.
                prices(2318, 45, 45, 10, 1),
                prices(2589, 10, 10, 26, 3),
                prices(2592, 40, 40, 6, 2),
                // Items: a buyout for two is 150 each.
                prices(4306, 150, 150, 2, 1),
                // Variants count as the plain item; the bid-only 7005 is left out.
                prices(11208, 1401, 1500, 3, 3),
                prices(19019, 4_000_000, 4_000_000, 2, 2),
            ]
        );
    }

    #[test]
    fn fingerprints_follow_the_auctions_not_their_order() {
        let a = r#"{"auctions":[
            {"id":1,"item":{"id":5},"quantity":1,"buyout":10},
            {"id":2,"item":{"id":5},"quantity":1,"buyout":20}]}"#;
        let swapped = r#"{"auctions":[
            {"id":2,"item":{"id":5},"quantity":1,"buyout":20},
            {"id":1,"item":{"id":5},"quantity":1,"buyout":10}]}"#;
        let other = r#"{"auctions":[
            {"id":1,"item":{"id":5},"quantity":1,"buyout":10},
            {"id":3,"item":{"id":5},"quantity":1,"buyout":20}]}"#;
        let print = |s: &str| snapshot(&[s]).unwrap().fingerprint;
        assert_eq!(print(a), print(swapped));
        assert_ne!(print(a), print(other));
    }

    #[test]
    fn empty_houses_have_no_items() {
        for empty in ["{}", r#"{"auctions":[]}"#] {
            assert_eq!(snapshot(&[empty]).unwrap().items, []);
        }
    }

    #[test]
    fn other_json_is_an_error() {
        assert!(snapshot(&["not json"]).is_err());
        assert!(snapshot(&[r#"{"auctions":[{"id":1}]}"#]).is_err());
    }

    #[test]
    fn a_day_from_its_snapshots() {
        let day = daily(
            7,
            &[
                prices(7, 90, 100, 10, 2),
                prices(7, 80, 120, 20, 3),
                prices(7, 95, 110, 3, 1),
            ],
        )
        .unwrap();
        assert_eq!(
            day,
            DailyPrices {
                item: 7,
                min_price: 80,
                median_price: 110,
                max_price: 120,
                quantity: 11,
                snapshots: 3,
            }
        );
        assert_eq!(daily(7, &[]), None);
    }

    #[test]
    fn money_reads_like_the_game() {
        assert_eq!(money(0), "0c");
        assert_eq!(money(45), "45c");
        assert_eq!(money(200), "2s 0c");
        assert_eq!(money(1_234), "12s 34c");
        assert_eq!(money(4_000_000), "400g 0s 0c");
        assert_eq!(money(10_203), "1g 2s 3c");
    }

    proptest! {
        #[test]
        fn money_adds_back_up(copper in 0u64..10_000_000_000) {
            let m = Money::from_copper(copper);
            prop_assert!(m.silver < 100 && m.copper < 100);
            prop_assert_eq!(m.gold * 10_000 + m.silver * 100 + m.copper, copper);
        }
    }

    #[test]
    fn brief_money_rounds_to_the_silver_from_a_gold_up() {
        let show = |m: Money| (m.gold, m.silver, m.shows_copper());
        assert_eq!(show(Money::brief(9_999)), (0, 99, true));
        assert_eq!(show(Money::brief(17_249)), (1, 72, false));
        assert_eq!(show(Money::brief(17_250)), (1, 73, false));
        assert_eq!(show(Money::brief(19_990)), (2, 0, false));
    }

    #[test]
    fn houses_have_labels() {
        let house = followed().next().unwrap();
        assert_eq!((house.id, house.label().as_str()), (110, "Alliance PvP"));
    }

    #[test]
    fn days_are_utc() {
        assert_eq!(day(0), 0);
        assert_eq!(day(DAY - 1), 0);
        assert_eq!(day(DAY), 1);
        assert_eq!(day(-1), -1);
    }

    proptest! {
        #[test]
        fn prices_stay_within_what_was_offered(
            offers in prop::collection::vec((1u32..4, 1u64..1_000_000, 1u64..200), 0..60)
        ) {
            let auctions: Vec<_> = offers.iter().enumerate().map(|(i, (item, price, quantity))| {
                serde_json::json!({"id": i, "item": {"id": item}, "quantity": quantity, "unit_price": price})
            }).collect();
            let dump = serde_json::json!({"auctions": auctions}).to_string();
            let snapshot = snapshot(&[&dump]).unwrap();
            for p in &snapshot.items {
                let mine: Vec<_> = offers.iter().filter(|(i, _, _)| *i == p.item).collect();
                prop_assert_eq!(p.auctions, mine.len() as u64);
                prop_assert_eq!(p.quantity, mine.iter().map(|(_, _, q)| q).sum::<u64>());
                prop_assert_eq!(p.min_price, mine.iter().map(|(_, price, _)| *price).min().unwrap());
                prop_assert!(p.min_price <= p.median_price);
                prop_assert!(mine.iter().any(|(_, price, _)| *price == p.median_price));
            }
            let items: std::collections::BTreeSet<_> = offers.iter().map(|(i, _, _)| *i).collect();
            prop_assert_eq!(snapshot.items.len(), items.len());
        }

        #[test]
        fn any_json_never_panics(s in ".*") {
            let _ = snapshot(&[&s]);
        }
    }
}
