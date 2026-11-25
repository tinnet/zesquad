//! The auction house overview: what moved, where the factions disagree, what sells for less
//! than a vendor pays, and the biggest markets. Pure ranking over prices read elsewhere.
//!
//! Forever's markets are thin and jumpy: one cheap stack bought out can turn a 2c median into
//! 4s overnight, which is real but not news. So moves compare finished days (the median of a
//! day's hourly medians, steadier than one snapshot), need some volume on both days, and moves
//! and gaps are ranked by the money they involve (the price difference times the units on
//! sale), not by how many times dearer: otherwise penny items top every list.

use std::collections::BTreeMap;

use crate::auctions::ItemPrices;

/// One item's typical price over a finished day in one house.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DayPrice {
    pub item: u32,
    /// The median of the day's snapshot medians, copper per unit.
    pub median: u64,
    /// Units on sale, on average over the day.
    pub quantity: u64,
    /// Snapshots the day had.
    pub snapshots: u64,
}

/// Units on sale both days for a move to count: fewer is one seller's whim.
pub const MIN_UNITS: u64 = 20;
/// Snapshots both days for a move to count: a day mostly missed isn't a day's price.
pub const MIN_SNAPSHOTS: u64 = 12;
/// Auctions on both sides for a faction gap to count.
pub const MIN_AUCTIONS: u64 = 5;

/// An item's price in one house from one day to the next. `house` is the caller's index.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Move {
    pub item: u32,
    pub house: usize,
    pub before: u64,
    pub now: u64,
    /// Units on sale, the fewer of the two days.
    pub quantity: u64,
}

impl Move {
    /// How many times the price it is now.
    #[must_use]
    pub fn ratio(&self) -> f64 {
        ratio(self.now, self.before)
    }

    /// The money the move involves: the price change times the units on sale. What moves are
    /// ranked by.
    #[must_use]
    pub fn weight(&self) -> u64 {
        self.now.abs_diff(self.before).saturating_mul(self.quantity)
    }
}

/// The items in `house` whose typical price changed from `before` to `now`, with enough on
/// sale both days to mean something.
#[must_use]
pub fn moves(house: usize, before: &[DayPrice], now: &[DayPrice]) -> Vec<Move> {
    let enough =
        |d: &DayPrice| d.quantity >= MIN_UNITS && d.snapshots >= MIN_SNAPSHOTS && d.median > 0;
    let before: BTreeMap<u32, &DayPrice> = before
        .iter()
        .filter(|d| enough(d))
        .map(|d| (d.item, d))
        .collect();
    now.iter()
        .filter(|d| enough(d))
        .filter_map(|d| {
            let b = before.get(&d.item)?;
            (b.median != d.median).then_some(Move {
                item: d.item,
                house,
                before: b.median,
                now: d.median,
                quantity: b.quantity.min(d.quantity),
            })
        })
        .collect()
}

/// The `n` biggest rises and the `n` biggest falls by [`Move::weight`], one per item (its
/// biggest in any house), biggest first.
#[must_use]
pub fn rising_and_falling(moves: Vec<Move>, n: usize) -> (Vec<Move>, Vec<Move>) {
    let (up, down): (Vec<Move>, Vec<Move>) = moves.into_iter().partition(|m| m.now > m.before);
    let mut up = best_per_item(up, |m| m.item, Move::weight);
    let mut down = best_per_item(down, |m| m.item, Move::weight);
    up.truncate(n);
    down.truncate(n);
    (up, down)
}

/// One item's price on both factions' houses of a pool.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Gap {
    pub item: u32,
    /// The caller's index of the pool.
    pub pool: usize,
    pub alliance: u64,
    pub horde: u64,
    /// Units on sale, the fewer of the two sides.
    pub quantity: u64,
}

impl Gap {
    /// How many times dearer the dearer side is.
    #[must_use]
    pub fn ratio(&self) -> f64 {
        ratio(self.alliance.max(self.horde), self.alliance.min(self.horde))
    }

    /// The money the gap involves: the price difference times the units on sale. What gaps
    /// are ranked by.
    #[must_use]
    pub fn weight(&self) -> u64 {
        self.alliance
            .abs_diff(self.horde)
            .saturating_mul(self.quantity)
    }
}

/// Items on sale on both factions' houses of `pool` with enough auctions on each, by
/// [`Gap::weight`], most first.
#[must_use]
pub fn gaps(pool: usize, alliance: &[ItemPrices], horde: &[ItemPrices]) -> Vec<Gap> {
    let horde: BTreeMap<u32, &ItemPrices> = horde
        .iter()
        .filter(|p| p.auctions >= MIN_AUCTIONS && p.median_price > 0)
        .map(|p| (p.item, p))
        .collect();
    let gaps = alliance
        .iter()
        .filter(|p| p.auctions >= MIN_AUCTIONS && p.median_price > 0)
        .filter_map(|a| {
            let h = horde.get(&a.item)?;
            (a.median_price != h.median_price).then_some(Gap {
                item: a.item,
                pool,
                alliance: a.median_price,
                horde: h.median_price,
                quantity: a.quantity.min(h.quantity),
            })
        })
        .collect();
    best_per_item(gaps, |g| g.item, Gap::weight)
}

/// The `n` biggest gaps by [`Gap::weight`] from several pools, one per item, biggest first.
#[must_use]
pub fn top_gaps(all: Vec<Gap>, n: usize) -> Vec<Gap> {
    let mut top = best_per_item(all, |g| g.item, Gap::weight);
    top.truncate(n);
    top
}

/// A listing a vendor would pay more for than it costs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bargain {
    pub item: u32,
    pub house: usize,
    /// The cheapest listing, per unit.
    pub price: u64,
    /// What a vendor pays for one.
    pub vendor: u64,
}

impl Bargain {
    /// Copper made per unit by buying it and selling it to a vendor.
    #[must_use]
    pub fn profit(&self) -> u64 {
        self.vendor.saturating_sub(self.price)
    }
}

/// Items in `house` whose cheapest listing is below what a vendor pays (`vendor`), most profit
/// per unit first.
#[must_use]
pub fn under_vendor(
    house: usize,
    prices: &[ItemPrices],
    vendor: impl Fn(u32) -> Option<u64>,
) -> Vec<Bargain> {
    let mut found: Vec<Bargain> = prices
        .iter()
        .filter_map(|p| {
            let v = vendor(p.item)?;
            (p.min_price < v).then_some(Bargain {
                item: p.item,
                house,
                price: p.min_price,
                vendor: v,
            })
        })
        .collect();
    found.sort_by(|a, b| b.profit().cmp(&a.profit()).then(a.item.cmp(&b.item)));
    found
}

/// The `n` best bargains from several houses, one per item (its most profitable), most profit
/// first.
#[must_use]
pub fn top_bargains(mut all: Vec<Bargain>, n: usize) -> Vec<Bargain> {
    all.sort_by(|a, b| b.profit().cmp(&a.profit()).then(a.item.cmp(&b.item)));
    let mut seen = std::collections::BTreeSet::new();
    all.retain(|b| seen.insert(b.item));
    all.truncate(n);
    all
}

/// One item's market across a region's houses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Market {
    pub item: u32,
    /// Copper's worth on sale.
    pub value: u64,
    pub quantity: u64,
}

/// The `n` items with the most worth on sale across `houses` (units times the median), most
/// first.
#[must_use]
pub fn biggest(houses: &[&[ItemPrices]], n: usize) -> Vec<Market> {
    let mut by_item: BTreeMap<u32, Market> = BTreeMap::new();
    for prices in houses {
        for p in *prices {
            let m = by_item.entry(p.item).or_insert(Market {
                item: p.item,
                value: 0,
                quantity: 0,
            });
            m.value = m
                .value
                .saturating_add(p.quantity.saturating_mul(p.median_price));
            m.quantity = m.quantity.saturating_add(p.quantity);
        }
    }
    let mut markets: Vec<Market> = by_item.into_values().collect();
    markets.sort_by(|a, b| b.value.cmp(&a.value).then(a.item.cmp(&b.item)));
    markets.truncate(n);
    markets
}

/// A change for people: `+35%` and `−20%` for small ones, `×2.5` and `÷225` from doubling or
/// halving up.
#[must_use]
pub fn change(before: u64, now: u64) -> String {
    let r = ratio(now, before);
    if r >= 2.0 {
        format!("×{}", times(r))
    } else if r <= 0.5 {
        format!("÷{}", times(1.0 / r))
    } else {
        let pct = ((r - 1.0) * 100.0).round();
        if pct >= 0.0 {
            format!("+{pct:.0}%")
        } else {
            format!("−{:.0}%", -pct)
        }
    }
}

/// A factor for people: one decimal under ten (`2.5`), whole from ten up (`225`).
#[must_use]
pub fn times(r: f64) -> String {
    if r < 10.0 {
        let s = format!("{r:.1}");
        s.strip_suffix(".0").map_or(s.clone(), str::to_owned)
    } else {
        format!("{r:.0}")
    }
}

/// `a / b` as a float, infinite for a zero `b`.
#[allow(clippy::cast_precision_loss)] // prices are far below 2^52 copper
fn ratio(a: u64, b: u64) -> f64 {
    if b == 0 {
        f64::INFINITY
    } else {
        a as f64 / b as f64
    }
}

/// One entry per item, the one with the highest `score`, highest first (ties by item).
fn best_per_item<T: Copy, S: Ord>(
    all: Vec<T>,
    item: impl Fn(&T) -> u32,
    score: impl Fn(&T) -> S,
) -> Vec<T> {
    let mut best: BTreeMap<u32, T> = BTreeMap::new();
    for t in all {
        match best.get(&item(&t)) {
            Some(b) if score(b) >= score(&t) => {}
            _ => {
                best.insert(item(&t), t);
            }
        }
    }
    let mut best: Vec<T> = best.into_values().collect();
    best.sort_by(|a, b| score(b).cmp(&score(a)).then(item(a).cmp(&item(b))));
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn day(item: u32, median: u64) -> DayPrice {
        DayPrice {
            item,
            median,
            quantity: 100,
            snapshots: 24,
        }
    }

    fn quote(item: u32, median: u64, auctions: u64) -> ItemPrices {
        ItemPrices {
            item,
            min_price: median / 2,
            median_price: median,
            quantity: auctions * 10,
            auctions,
        }
    }

    #[test]
    fn moves_need_volume_on_both_days() {
        let before = [
            day(1, 100),
            day(2, 100),
            DayPrice {
                quantity: 5,
                ..day(3, 100)
            },
            day(4, 100),
            DayPrice {
                snapshots: 3,
                ..day(5, 100)
            },
        ];
        let now = [
            day(1, 300),
            day(2, 100),
            day(3, 900),
            day(4, 50),
            day(5, 900),
            day(6, 900),
        ];
        let found: Vec<(u32, u64, u64)> = moves(0, &before, &now)
            .iter()
            .map(|m| (m.item, m.before, m.now))
            .collect();
        assert_eq!(
            found,
            [(1, 100, 300), (4, 100, 50)],
            "2 didn't move, 3 and 5 thin, 6 new"
        );
    }

    #[test]
    fn moves_are_ranked_by_the_money_involved() {
        let m = |item, before, now, quantity| Move {
            item,
            house: 0,
            before,
            now,
            quantity,
        };
        // A penny item up a hundredfold on 20 units moves 19s 80c; a bar up 5% on 1,500 units
        // moves 2,250g.
        let (up, _) = rising_and_falling(vec![m(1, 1, 100, 20), m(2, 300_000, 315_000, 1_500)], 2);
        assert_eq!(up.iter().map(|m| m.item).collect::<Vec<_>>(), [2, 1]);
        assert_eq!(up[0].weight(), 22_500_000);
    }

    #[test]
    fn rises_and_falls_are_ranked_once_per_item() {
        let m = |item, house, before, now| Move {
            item,
            house,
            before,
            now,
            quantity: 100,
        };
        let all = vec![
            m(1, 0, 100, 150),
            m(1, 1, 100, 400), // item 1's biggest rise is in house 1
            m(2, 0, 100, 200),
            m(3, 0, 100, 10),
            m(3, 1, 100, 300), // item 3 rose in one house and fell in another
            m(4, 0, 100, 80),
        ];
        let (up, down) = rising_and_falling(all, 2);
        assert_eq!(
            up.iter().map(|m| (m.item, m.house)).collect::<Vec<_>>(),
            [(1, 1), (3, 1)]
        );
        assert_eq!(
            down.iter().map(|m| (m.item, m.house)).collect::<Vec<_>>(),
            [(3, 0), (4, 0)]
        );
    }

    #[test]
    fn gaps_need_auctions_on_both_sides() {
        let alliance = [
            quote(1, 100, 10),
            quote(2, 100, 10),
            quote(3, 100, 2),
            quote(4, 500, 10),
        ];
        let horde = [
            quote(1, 300, 10),
            quote(2, 110, 10),
            quote(3, 900, 10),
            quote(4, 100, 9),
        ];
        let found: Vec<(u32, u64, u64)> = gaps(1, &alliance, &horde)
            .iter()
            .map(|g| (g.item, g.alliance, g.horde))
            .collect();
        assert_eq!(found, [(4, 500, 100), (1, 100, 300), (2, 100, 110)]);
        assert!((gaps(1, &alliance, &horde)[0].ratio() - 5.0).abs() < 1e-9);
    }

    #[test]
    fn bargains_are_below_the_vendor_price() {
        let prices = [
            quote(1, 200, 5),
            quote(2, 2_000, 5),
            quote(3, 100, 5),
            quote(4, 50, 5),
        ];
        let vendor = |item| match item {
            1 => Some(150),   // cheapest listing 100: 50 profit
            2 => Some(1_500), // cheapest 1,000: 500 profit
            3 => Some(50),    // cheapest 50: no profit
            _ => None,
        };
        let found: Vec<(u32, u64)> = under_vendor(2, &prices, vendor)
            .iter()
            .map(|b| (b.item, b.profit()))
            .collect();
        assert_eq!(found, [(2, 500), (1, 50)]);
    }

    #[test]
    fn tops_keep_each_items_best() {
        let g = |item, pool, alliance, horde| Gap {
            item,
            pool,
            alliance,
            horde,
            quantity: 10,
        };
        let top = top_gaps(
            vec![g(1, 0, 100, 200), g(1, 1, 100, 900), g(2, 0, 500, 100)],
            5,
        );
        assert_eq!(top, [g(1, 1, 100, 900), g(2, 0, 500, 100)]);
        let b = |item, house, price, vendor| Bargain {
            item,
            house,
            price,
            vendor,
        };
        let top = top_bargains(
            vec![
                b(1, 0, 10, 50),
                b(1, 2, 5, 50),
                b(2, 1, 10, 30),
                b(3, 1, 1, 2),
            ],
            2,
        );
        assert_eq!(top, [b(1, 2, 5, 50), b(2, 1, 10, 30)]);
    }

    #[test]
    fn biggest_markets_add_up_across_houses() {
        let a = [quote(1, 100, 10), quote(2, 1_000, 1)];
        let b = [quote(1, 100, 10), quote(3, 5, 10)];
        let found = biggest(&[&a, &b], 2);
        assert_eq!(
            found,
            [
                Market {
                    item: 1,
                    value: 20_000,
                    quantity: 200
                },
                Market {
                    item: 2,
                    value: 10_000,
                    quantity: 10
                },
            ]
        );
    }

    #[test]
    fn changes_read_well() {
        assert_eq!(change(100, 135), "+35%");
        assert_eq!(change(100, 80), "−20%");
        assert_eq!(change(100, 250), "×2.5");
        assert_eq!(change(100, 200), "×2");
        assert_eq!(change(2, 450), "×225");
        assert_eq!(change(450, 2), "÷225");
        assert_eq!(change(100, 40), "÷2.5");
        assert_eq!(change(100, 100), "+0%");
    }

    proptest! {
        #[test]
        fn rankings_are_sorted_and_unique(
            raw in prop::collection::vec(
                (1u32..20, 0usize..4, 1u64..10_000, 1u64..10_000, 1u64..1_000),
                0..80,
            ),
            n in 0usize..10,
        ) {
            let all: Vec<Move> = raw
                .into_iter()
                .filter(|&(_, _, b, a, _)| a != b)
                .map(|(item, house, before, now, quantity)| Move { item, house, before, now, quantity })
                .collect();
            let (up, down) = rising_and_falling(all, n);
            prop_assert!(up.len() <= n && down.len() <= n);
            prop_assert!(up.iter().all(|m| m.now > m.before));
            prop_assert!(down.iter().all(|m| m.now < m.before));
            prop_assert!(up.windows(2).all(|w| w[0].weight() >= w[1].weight()));
            prop_assert!(down.windows(2).all(|w| w[0].weight() >= w[1].weight()));
            let mut items: Vec<u32> = up.iter().map(|m| m.item).collect();
            items.sort_unstable();
            items.dedup();
            prop_assert_eq!(items.len(), up.len());
        }

        #[test]
        fn changes_never_panic_and_say_which_way(before in 1u64..u64::MAX / 2, now in 1u64..u64::MAX / 2) {
            let c = change(before, now);
            if now > before * 2 {
                prop_assert!(c.starts_with('×'), "{}", c);
            }
            if now * 2 < before {
                prop_assert!(c.starts_with('÷'), "{}", c);
            }
        }
    }
}
