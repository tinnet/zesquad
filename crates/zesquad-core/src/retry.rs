//! How long to leave Battle.net alone after it answers 429 Too Many Requests.

use std::time::{Duration, SystemTime};

/// When a 429 doesn't say how long, or says it in a way we can't read. Battle.net's per-second
/// limit lasts the rest of that second, so this is plenty.
pub const DEFAULT: Duration = Duration::from_secs(10);
/// The longest wait taken from a `Retry-After`, so a broken one can't stop lookups for good.
pub const MAX: Duration = Duration::from_hours(1);

/// How long to wait from a 429's `Retry-After` at `now`: whole seconds or an HTTP date
/// (RFC 9110), [`DEFAULT`] when missing or unreadable, at most [`MAX`]. A date in the past
/// means no wait.
#[must_use]
pub fn retry_after(header: Option<&str>, now: SystemTime) -> Duration {
    let Some(value) = header.map(str::trim) else {
        return DEFAULT;
    };
    let wait = if let Ok(secs) = value.parse::<u64>() {
        Duration::from_secs(secs)
    } else if let Ok(at) = httpdate::parse_http_date(value) {
        at.duration_since(now).unwrap_or(Duration::ZERO)
    } else {
        DEFAULT
    };
    wait.min(MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn at(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
    }

    #[test]
    fn reads_seconds_and_dates() {
        let now = at(1_795_206_228); // Fri, 20 Nov 2026 20:23:48 GMT
        let secs = Duration::from_secs;
        assert_eq!(retry_after(Some("3"), now), secs(3));
        assert_eq!(retry_after(Some(" 0 "), now), secs(0));
        assert_eq!(
            retry_after(Some("Fri, 20 Nov 2026 20:24:18 GMT"), now),
            secs(30)
        );
        assert_eq!(
            retry_after(Some("Fri, 20 Nov 2026 20:20:00 GMT"), now),
            secs(0),
            "already past"
        );
    }

    #[test]
    fn falls_back_and_caps() {
        let now = at(0);
        assert_eq!(retry_after(None, now), DEFAULT);
        assert_eq!(retry_after(Some("soon"), now), DEFAULT);
        assert_eq!(retry_after(Some("-5"), now), DEFAULT);
        assert_eq!(retry_after(Some("999999"), now), MAX);
    }

    proptest! {
        #[test]
        fn never_waits_longer_than_the_cap(s in ".*", now in 0u64..4_000_000_000) {
            prop_assert!(retry_after(Some(&s), at(now)) <= MAX);
        }
    }
}
