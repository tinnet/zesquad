//! Where raw auction dumps are filed: one object per dump, named by house and by when
//! Battle.net made it, so a listing sorts by time and a day's dumps share a prefix.

use std::fmt;

use crate::Region;
use crate::auctions::DAY;

/// A UTC time broken into calendar fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Utc {
    pub year: i64,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

impl Utc {
    /// The calendar date and time of a Unix time in seconds (proleptic Gregorian).
    #[must_use]
    pub fn from_unix(at: i64) -> Self {
        let days = at.div_euclid(DAY);
        let secs = at.rem_euclid(DAY);
        // Howard Hinnant's civil_from_days: eras of 400 years, years starting in March.
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = doy - (153 * mp + 2) / 5 + 1;
        let month = if mp < 10 { mp + 3 } else { mp - 9 };
        let year = yoe + era * 400 + i64::from(month <= 2);
        // Every field below is in range by construction (day 1-31, month 1-12, secs < DAY).
        let small = |n: i64| u32::try_from(n).unwrap_or_default();
        Self {
            year,
            month: small(month),
            day: small(day),
            hour: small(secs / 3600),
            minute: small(secs % 3600 / 60),
            second: small(secs % 60),
        }
    }

    /// `2026-11-20`.
    #[must_use]
    pub fn date(self) -> String {
        format!("{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }

    /// `20261120` (as S3 signatures want the date).
    #[must_use]
    pub fn compact_date(self) -> String {
        format!("{:04}{:02}{:02}", self.year, self.month, self.day)
    }
}

/// ISO 8601 basic: `20261120T202348Z` (as in S3's `x-amz-date`).
impl fmt::Display for Utc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}T{:02}{:02}{:02}Z",
            self.compact_date(),
            self.hour,
            self.minute,
            self.second
        )
    }
}

/// The object key of a dump made at `at` (Unix seconds), e.g.
/// `eu/110/2026-11-20/20261120T202348Z-commodities.json.zst`.
#[must_use]
pub fn dump_key(region: Region, house: u32, commodities: bool, at: i64) -> String {
    let utc = Utc::from_unix(at);
    let kind = if commodities { "commodities" } else { "items" };
    format!("{region}/{house}/{}/{utc}-{kind}.json.zst", utc.date())
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn known_times() {
        let utc = Utc::from_unix(1_795_206_228);
        assert_eq!(utc.to_string(), "20261120T202348Z");
        assert_eq!(utc.date(), "2026-11-20");
        assert_eq!(Utc::from_unix(0).to_string(), "19700101T000000Z");
        assert_eq!(Utc::from_unix(951_782_400).date(), "2000-02-29");
        assert_eq!(Utc::from_unix(-1).to_string(), "19691231T235959Z");
    }

    #[test]
    fn keys_sort_by_house_then_time() {
        assert_eq!(
            dump_key(Region::Eu, 110, true, 1_795_206_228),
            "eu/110/2026-11-20/20261120T202348Z-commodities.json.zst"
        );
        assert_eq!(
            dump_key(Region::Us, 113, false, 1_795_206_228),
            "us/113/2026-11-20/20261120T202348Z-items.json.zst"
        );
    }

    proptest! {
        #[test]
        fn later_times_sort_later(a in 0i64..8_000_000_000, b in 0i64..8_000_000_000) {
            let (ka, kb) = (Utc::from_unix(a).to_string(), Utc::from_unix(b).to_string());
            prop_assert_eq!(a.cmp(&b), ka.cmp(&kb));
        }

        #[test]
        fn days_follow_each_other(day in -100_000i64..100_000) {
            let (today, tomorrow) = (Utc::from_unix(day * DAY), Utc::from_unix((day + 1) * DAY));
            prop_assert!(tomorrow.date() > today.date() || today.year < 0);
            prop_assert!((1..=12).contains(&today.month) && (1..=31).contains(&today.day));
            let next = if today.day < 28 {
                Some((today.year, today.month, today.day + 1))
            } else {
                None
            };
            if let Some(next) = next {
                prop_assert_eq!((tomorrow.year, tomorrow.month, tomorrow.day), next);
            } else {
                prop_assert!(tomorrow.day == 1 || tomorrow.day == today.day + 1);
            }
        }
    }
}
