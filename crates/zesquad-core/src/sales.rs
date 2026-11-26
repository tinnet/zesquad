//! Sales, estimated by matching auction ids from one dump of a house to the next.
//!
//! A dump lists every auction with an id that stays the same while it's up. An auction gone by
//! the next dump was bought, cancelled or expired. Battle.net only says roughly how long each had
//! left (`time_left`), so one that had more time left than passed between the dumps can't have
//! expired: it counts as sold. Cancelled auctions can't be told apart and count too, so these are
//! estimates, on the high side for items people undercut and relist often. A stack that shrank
//! (part of a commodity auction bought) is a sale for certain.
//!
//! Sales listed and bought between two dumps are never seen, and auctions that could have expired
//! are left out, so the estimate is on the low side for items that sell within minutes.

use serde::Deserialize;

use crate::auctions::DAY;

/// Roughly how long an auction has left: Battle.net's `time_left`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TimeLeft {
    Short,
    Medium,
    Long,
    VeryLong,
    /// Missing, or a value we don't know: treated as about to expire.
    #[default]
    #[serde(other)]
    Unknown,
}

impl TimeLeft {
    /// The least time left, in seconds: under 30 minutes, 30 minutes to 2 hours, 2 to 12 hours,
    /// over 12 hours (the game's bands; see Auction houses in `docs/battlenet-api.md`).
    #[must_use]
    pub fn at_least(self) -> i64 {
        match self {
            Self::Short | Self::Unknown => 0,
            Self::Medium => 30 * 60,
            Self::Long => 2 * 3_600,
            Self::VeryLong => 12 * 3_600,
        }
    }

    fn code(self) -> u8 {
        match self {
            Self::Unknown => 0,
            Self::Short => 1,
            Self::Medium => 2,
            Self::Long => 3,
            Self::VeryLong => 4,
        }
    }

    fn from_code(code: u8) -> Option<Self> {
        Some(match code {
            0 => Self::Unknown,
            1 => Self::Short,
            2 => Self::Medium,
            3 => Self::Long,
            4 => Self::VeryLong,
            _ => return None,
        })
    }
}

/// One auction in a dump.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Listing {
    pub id: u64,
    pub item: u32,
    pub quantity: u64,
    /// Copper per unit; `None` for bid-only auctions.
    pub unit_price: Option<u64>,
    pub time_left: TimeLeft,
}

/// Dumps further apart than this aren't compared: auctions with 2 to 12 hours left could have
/// expired in between, so most sales would be missed.
pub const MAX_GAP: i64 = 2 * 3_600;

/// One item's sales between two dumps, or summed over several.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemSales {
    pub item: u32,
    /// Units sold.
    pub sold: u64,
    /// What they sold for, copper. Bid-only auctions add nothing.
    pub value: u64,
}

/// Sales over a stretch of time: `seconds` of it compared, and what sold, by item.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sales {
    pub seconds: i64,
    pub items: Vec<ItemSales>,
}

/// What sold between a dump's listings (`before`) and the next one's (`after`), `seconds` apart,
/// both sorted by id. `None` when the dumps are too far apart to compare (see [`MAX_GAP`]) or
/// the second isn't later.
#[must_use]
pub fn compare(before: &[Listing], after: &[Listing], seconds: i64) -> Option<Sales> {
    if !(1..=MAX_GAP).contains(&seconds) {
        return None;
    }
    let mut by_item: std::collections::BTreeMap<u32, ItemSales> = std::collections::BTreeMap::new();
    let mut later = after.iter().peekable();
    for was in before {
        while later.next_if(|l| l.id < was.id).is_some() {}
        let sold = match later.next_if(|l| l.id == was.id) {
            // Still up: part of a stack may have been bought.
            Some(now) => was.quantity.saturating_sub(now.quantity),
            None if was.time_left.at_least() >= seconds => was.quantity,
            // Gone, but it may have expired.
            None => 0,
        };
        if sold > 0 {
            let entry = by_item.entry(was.item).or_insert(ItemSales {
                item: was.item,
                sold: 0,
                value: 0,
            });
            entry.sold = entry.sold.saturating_add(sold);
            let value = was.unit_price.unwrap_or(0).saturating_mul(sold);
            entry.value = entry.value.saturating_add(value);
        }
    }
    Some(Sales {
        seconds,
        items: by_item.into_values().collect(),
    })
}

/// `units` over `seconds` as a rate per day; `None` without any time.
#[must_use]
#[allow(clippy::cast_precision_loss)] // counts far below 2^52
pub fn per_day(units: u64, seconds: i64) -> Option<f64> {
    (seconds > 0).then(|| units as f64 * DAY as f64 / seconds as f64)
}

/// A rate per day for people: `0`, `<1`, then like [`crate::market::times`] (`2.5`, `40`).
#[must_use]
pub fn rate(per_day: f64) -> String {
    if per_day <= 0.0 {
        "0".to_owned()
    } else if per_day < 1.0 {
        "<1".to_owned()
    } else {
        crate::market::times(per_day)
    }
}

/// The format of [`encode`]'s first byte, changed whenever the layout does.
const VERSION: u8 = 1;

/// Packs listings sorted by id into bytes, to keep until the next dump: about 10 bytes each
/// (the ids as the gaps between them, every number as a varint), where JSON takes over 100.
#[must_use]
pub fn encode(listings: &[Listing]) -> Vec<u8> {
    let mut out = Vec::with_capacity(listings.len() * 10 + 10);
    out.push(VERSION);
    put(&mut out, listings.len() as u64);
    let mut last = 0;
    for l in listings {
        put(&mut out, l.id.wrapping_sub(last));
        last = l.id;
        put(&mut out, u64::from(l.item));
        put(&mut out, l.quantity);
        put(&mut out, l.unit_price.map_or(0, |p| p.saturating_add(1)));
        out.push(l.time_left.code());
    }
    out
}

/// Bytes that aren't [`encode`]'s.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("listings in an unknown format")]
pub struct DecodeError;

/// Reads [`encode`]'s bytes back.
///
/// # Errors
///
/// When the bytes aren't [`encode`]'s, of this version.
pub fn decode(bytes: &[u8]) -> Result<Vec<Listing>, DecodeError> {
    let (&version, mut rest) = bytes.split_first().ok_or(DecodeError)?;
    if version != VERSION {
        return Err(DecodeError);
    }
    let count = take(&mut rest)?;
    // Each listing takes at least 5 bytes, so a count beyond that is a lie: don't trust it
    // for the allocation.
    let mut out = Vec::with_capacity(usize::try_from(count).unwrap_or(0).min(rest.len() / 5));
    let mut last: u64 = 0;
    for _ in 0..count {
        let id = last.wrapping_add(take(&mut rest)?);
        last = id;
        let item = u32::try_from(take(&mut rest)?).map_err(|_| DecodeError)?;
        let quantity = take(&mut rest)?;
        let unit_price = take(&mut rest)?.checked_sub(1);
        let (&code, tail) = rest.split_first().ok_or(DecodeError)?;
        rest = tail;
        let time_left = TimeLeft::from_code(code).ok_or(DecodeError)?;
        out.push(Listing {
            id,
            item,
            quantity,
            unit_price,
            time_left,
        });
    }
    if rest.is_empty() {
        Ok(out)
    } else {
        Err(DecodeError)
    }
}

/// Appends `n` as a LEB128 varint.
fn put(out: &mut Vec<u8>, mut n: u64) {
    while n >= 0x80 {
        out.push(u8::try_from(n & 0x7f).unwrap_or(0) | 0x80);
        n >>= 7;
    }
    out.push(u8::try_from(n).unwrap_or(0));
}

/// Takes a LEB128 varint off the front of `bytes`.
fn take(bytes: &mut &[u8]) -> Result<u64, DecodeError> {
    let mut n: u64 = 0;
    for shift in (0..64).step_by(7) {
        let (&b, rest) = bytes.split_first().ok_or(DecodeError)?;
        *bytes = rest;
        let part = u64::from(b & 0x7f);
        if shift == 63 && part > 1 {
            return Err(DecodeError);
        }
        n |= part << shift;
        if b & 0x80 == 0 {
            return Ok(n);
        }
    }
    Err(DecodeError)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const HOUR: i64 = 3_600;

    fn listing(id: u64, item: u32, quantity: u64, price: u64, time_left: TimeLeft) -> Listing {
        Listing {
            id,
            item,
            quantity,
            unit_price: Some(price),
            time_left,
        }
    }

    #[test]
    fn gone_with_time_to_spare_is_sold() {
        use TimeLeft::{Long, Medium, Short, Unknown, VeryLong};
        let before = [
            listing(1, 7, 1, 100, Long),
            listing(2, 7, 1, 120, VeryLong),
            listing(3, 7, 1, 90, Short),
            listing(4, 7, 1, 80, Medium),
            listing(5, 7, 1, 70, Unknown),
            listing(6, 8, 1, 500, Long),
        ];
        // 6 is still up; the rest are gone an hour later.
        let after = [listing(6, 8, 1, 500, Long), listing(9, 7, 1, 95, VeryLong)];
        let sales = compare(&before, &after, HOUR).unwrap();
        assert_eq!(
            sales,
            Sales {
                seconds: HOUR,
                // Only 1 and 2 couldn't have expired in an hour.
                items: vec![ItemSales {
                    item: 7,
                    sold: 2,
                    value: 220
                }],
            }
        );
        // Over 20 minutes, one with 30 minutes to 2 hours left couldn't have expired either.
        assert_eq!(compare(&before, &after, 20 * 60).unwrap().items[0].sold, 3);
    }

    #[test]
    fn a_stack_that_shrank_sold_the_difference() {
        let before = [listing(1, 2589, 20, 10, TimeLeft::Short)];
        let after = [listing(1, 2589, 5, 10, TimeLeft::Short)];
        assert_eq!(
            compare(&before, &after, HOUR).unwrap().items,
            [ItemSales {
                item: 2589,
                sold: 15,
                value: 150
            }]
        );
    }

    #[test]
    fn bid_only_auctions_sell_for_nothing_known() {
        let mut gone = listing(1, 7, 2, 0, TimeLeft::Long);
        gone.unit_price = None;
        assert_eq!(
            compare(&[gone], &[], HOUR).unwrap().items,
            [ItemSales {
                item: 7,
                sold: 2,
                value: 0
            }]
        );
    }

    #[test]
    fn dumps_too_far_apart_or_out_of_order_arent_compared() {
        let before = [listing(1, 7, 1, 100, TimeLeft::VeryLong)];
        assert!(compare(&before, &[], MAX_GAP).is_some());
        assert_eq!(compare(&before, &[], MAX_GAP + 1), None);
        assert_eq!(compare(&before, &[], 0), None);
        assert_eq!(compare(&before, &[], -HOUR), None);
    }

    #[test]
    fn rates_per_day() {
        assert_eq!(per_day(5, 12 * HOUR), Some(10.0));
        assert_eq!(per_day(5, 0), None);
        assert_eq!(rate(0.0), "0");
        assert_eq!(rate(0.4), "<1");
        assert_eq!(rate(2.5), "2.5");
        assert_eq!(rate(41.2), "41");
    }

    #[test]
    fn other_bytes_dont_decode() {
        assert_eq!(decode(&[]), Err(DecodeError));
        assert_eq!(decode(&[2, 0]), Err(DecodeError), "another version");
        let mut bytes = encode(&[listing(1, 7, 1, 100, TimeLeft::Long)]);
        bytes.push(0);
        assert_eq!(decode(&bytes), Err(DecodeError), "left over");
        bytes.truncate(bytes.len() - 2);
        assert_eq!(decode(&bytes), Err(DecodeError), "cut short");
    }

    fn listings() -> impl Strategy<Value = Vec<Listing>> {
        let one = (
            1u32..6,
            1u64..30,
            prop::option::of(0u64..u64::MAX / 2),
            0u8..5,
        );
        prop::collection::btree_map(any::<u64>(), one, 0..80).prop_map(|m| {
            m.into_iter()
                .map(|(id, (item, quantity, unit_price, code))| Listing {
                    id,
                    item,
                    quantity,
                    unit_price,
                    time_left: TimeLeft::from_code(code).unwrap(),
                })
                .collect()
        })
    }

    proptest! {
        #[test]
        fn listings_read_back(l in listings()) {
            prop_assert_eq!(decode(&encode(&l)).unwrap(), l);
        }

        #[test]
        fn any_bytes_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..200)) {
            let _ = decode(&bytes);
        }

        #[test]
        fn nothing_sells_when_nothing_changed(l in listings(), seconds in 1..=MAX_GAP) {
            prop_assert_eq!(compare(&l, &l, seconds).unwrap().items, []);
        }

        #[test]
        fn no_more_sells_than_was_up(
            before in listings(),
            after in listings(),
            seconds in 1..=MAX_GAP,
        ) {
            let sales = compare(&before, &after, seconds).unwrap();
            for s in &sales.items {
                let up: u64 = before.iter().filter(|l| l.item == s.item).map(|l| l.quantity).sum();
                prop_assert!(s.sold > 0 && s.sold <= up);
            }
            let items: Vec<u32> = sales.items.iter().map(|s| s.item).collect();
            prop_assert!(items.windows(2).all(|w| w[0] < w[1]), "by item, once each");
        }
    }
}
