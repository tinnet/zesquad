//! Slowing down repeated requests: how many of a kind one client may make in a window of time,
//! and who "one client" is. The server keeps the counters in memory; this is the arithmetic.

use std::collections::HashMap;
use std::hash::Hash;
use std::net::IpAddr;

/// At most `max` requests per `window` seconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rule {
    pub max: u32,
    pub window: u64,
}

/// Counts requests per key in fixed windows: each key's window starts at its first request
/// and lasts `rule.window` seconds. Simple, and at worst lets through twice `max` across a
/// window boundary, which is fine for slowing people down.
#[derive(Debug)]
pub struct Counter<K> {
    rule: Rule,
    /// Per key: when its window started, and how many requests it has had.
    windows: HashMap<K, (u64, u32)>,
    /// When finished windows were last forgotten.
    pruned_at: Option<u64>,
}

/// Over the limit; try again in this many seconds (at least 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryAfter(pub u64);

/// Forgetting finished windows once there are this many keys keeps memory bounded by the
/// clients seen in one window, not since start. It happens at most once a second, so a flood
/// of clients still inside their window doesn't make every request scan them all.
const PRUNE_AT: usize = 10_000;

impl<K: Eq + Hash> Counter<K> {
    #[must_use]
    pub fn new(rule: Rule) -> Self {
        Self {
            rule,
            windows: HashMap::new(),
            pruned_at: None,
        }
    }

    /// Counts a request from `key` at `now` (seconds), unless it's over the limit.
    ///
    /// # Errors
    ///
    /// Over the limit: how long until the window ends. Refused requests aren't counted.
    pub fn hit(&mut self, key: K, now: u64) -> Result<(), RetryAfter> {
        if self.windows.len() >= PRUNE_AT && self.pruned_at != Some(now) {
            self.prune(now);
        }
        let window = self.rule.window;
        let (start, count) = self.windows.entry(key).or_insert((now, 0));
        if now.saturating_sub(*start) >= window || now < *start {
            *start = now;
            *count = 0;
        }
        if *count >= self.rule.max {
            return Err(RetryAfter((*start + window).saturating_sub(now).max(1)));
        }
        *count += 1;
        Ok(())
    }

    /// Forgets keys whose window has ended.
    pub fn prune(&mut self, now: u64) {
        self.pruned_at = Some(now);
        let window = self.rule.window;
        self.windows
            .retain(|_, (start, _)| now.saturating_sub(*start) < window);
    }

    /// Keys being counted, for tests.
    #[must_use]
    pub fn len(&self) -> usize {
        self.windows.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.windows.is_empty()
    }
}

/// Who counts as one client: an IPv4 address, or an IPv6 /64, since one home or phone gets a
/// whole /64 and can pick any address in it.
#[must_use]
pub fn client_key(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V4(v4) => IpAddr::V4(v4),
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or_else(
            || IpAddr::V6((u128::from(v6) & !((1u128 << 64) - 1)).into()),
            IpAddr::V4,
        ),
    }
}

/// The client's address from Cloudflare's `CF-Connecting-IP`. Only Cloudflare sets it; any
/// other proxy passes on whatever the client sent.
#[must_use]
pub fn cloudflare_client(cf_connecting_ip: Option<&str>) -> Option<IpAddr> {
    cf_connecting_ip?.trim().parse().ok()
}

/// The client's address from `X-Forwarded-For`, given every value of the header in order: the
/// last entry of the last value, the one the proxy in front added. Entries before it are
/// whatever the client sent, whether the proxy appends to the client's header or adds a header
/// of its own (`HAProxy`).
#[must_use]
pub fn forwarded_for_client<'a>(values: impl IntoIterator<Item = &'a str>) -> Option<IpAddr> {
    values
        .into_iter()
        .last()?
        .rsplit(',')
        .next()?
        .trim()
        .parse()
        .ok()
}

/// "a minute", "5 minutes", "an hour": how long to wait, rounded up to whole minutes.
#[must_use]
pub fn wait_text(seconds: u64) -> String {
    match seconds.div_ceil(60) {
        0 | 1 => "a minute".to_owned(),
        60 => "an hour".to_owned(),
        minutes => format!("{minutes} minutes"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const RULE: Rule = Rule { max: 3, window: 60 };

    #[test]
    fn allows_max_per_window_then_says_when_to_retry() {
        let mut c = Counter::new(RULE);
        for t in [100, 101, 102] {
            assert_eq!(c.hit("a", t), Ok(()));
        }
        assert_eq!(c.hit("a", 110), Err(RetryAfter(50)));
        // Others aren't affected.
        assert_eq!(c.hit("b", 110), Ok(()));
        // A new window.
        assert_eq!(c.hit("a", 160), Ok(()));
        assert_eq!(c.hit("a", 159 + 60), Ok(()));
        assert_eq!(c.hit("a", 219), Ok(()));
        assert_eq!(c.hit("a", 219), Err(RetryAfter(1)));
    }

    #[test]
    fn a_clock_going_back_starts_over() {
        let mut c = Counter::new(RULE);
        for _ in 0..3 {
            c.hit("a", 100).unwrap();
        }
        assert_eq!(c.hit("a", 50), Ok(()));
    }

    #[test]
    fn finished_windows_are_forgotten() {
        let mut c = Counter::new(RULE);
        c.hit(1, 0).unwrap();
        c.hit(2, 30).unwrap();
        c.prune(60);
        assert_eq!(c.len(), 1);
        c.prune(90);
        assert!(c.is_empty());
    }

    #[test]
    fn memory_stays_bounded() {
        let mut c = Counter::new(Rule { max: 1, window: 1 });
        for i in 0..(PRUNE_AT as u64 * 3) {
            c.hit(i, i / 100).unwrap();
        }
        assert!(c.len() <= PRUNE_AT, "{}", c.len());
    }

    #[test]
    fn ipv6_counts_per_64() {
        let a: IpAddr = "2001:db8:1:2:aaaa::1".parse().unwrap();
        let b: IpAddr = "2001:db8:1:2:ffff::9".parse().unwrap();
        let c: IpAddr = "2001:db8:1:3::1".parse().unwrap();
        assert_eq!(client_key(a), client_key(b));
        assert_ne!(client_key(a), client_key(c));
        assert_eq!(client_key(a), "2001:db8:1:2::".parse::<IpAddr>().unwrap());
        let v4: IpAddr = "192.0.2.7".parse().unwrap();
        assert_eq!(client_key(v4), v4);
        assert_eq!(client_key("::ffff:192.0.2.7".parse().unwrap()), v4);
    }

    #[test]
    fn reads_the_proxys_word_only() {
        let ip = |s: &str| Some(s.parse::<IpAddr>().unwrap());
        assert_eq!(cloudflare_client(Some(" 192.0.2.7 ")), ip("192.0.2.7"));
        assert_eq!(cloudflare_client(Some("nonsense")), None);
        assert_eq!(cloudflare_client(None), None);
        // The client can send anything first; the proxy's entry is the last, whether it
        // appended to the client's header or added its own.
        assert_eq!(
            forwarded_for_client(["1.2.3.4, 198.51.100.9"]),
            ip("198.51.100.9")
        );
        assert_eq!(
            forwarded_for_client(["1.2.3.4", "2001:db8::1"]),
            ip("2001:db8::1")
        );
        assert_eq!(forwarded_for_client(["1.2.3.4, junk"]), None);
        assert_eq!(forwarded_for_client([]), None);
    }

    #[test]
    fn says_how_long_to_wait() {
        assert_eq!(wait_text(1), "a minute");
        assert_eq!(wait_text(60), "a minute");
        assert_eq!(wait_text(61), "2 minutes");
        assert_eq!(wait_text(3500), "59 minutes");
        assert_eq!(wait_text(3600), "an hour");
    }

    #[test]
    fn a_crowd_inside_its_window_is_scanned_once_a_second() {
        let mut c = Counter::new(Rule { max: 1, window: 10 });
        for i in 0..PRUNE_AT as u64 {
            c.hit(i, 100).unwrap();
        }
        c.hit(u64::MAX, 105).unwrap();
        // A finished window, slipped in after that scan.
        c.windows.insert(u64::MAX - 1, (0, 1));
        c.hit(u64::MAX - 2, 105).unwrap();
        assert!(
            c.windows.contains_key(&(u64::MAX - 1)),
            "not again within the second"
        );
        c.hit(u64::MAX - 3, 106).unwrap();
        assert!(!c.windows.contains_key(&(u64::MAX - 1)));
    }

    proptest! {
        #[test]
        fn never_more_than_max_in_a_window(times in prop::collection::vec(0u64..300, 0..200)) {
            let mut times = times;
            times.sort_unstable();
            let mut c = Counter::new(RULE);
            let allowed: Vec<u64> = times.into_iter().filter(|&t| c.hit((), t).is_ok()).collect();
            // Fixed windows: any stretch of `window` seconds overlaps at most two of them.
            for (i, &t) in allowed.iter().enumerate() {
                let within = allowed[i..].iter().take_while(|&&u| u < t + RULE.window).count();
                prop_assert!(within <= 2 * RULE.max as usize);
            }
        }

        #[test]
        fn retry_after_is_honest(extra in 0u64..60) {
            let mut c = Counter::new(RULE);
            for _ in 0..RULE.max {
                c.hit((), 1000).unwrap();
            }
            let now = 1000 + extra;
            let wait = c.hit((), now).unwrap_err().0;
            prop_assert!(wait >= 1);
            prop_assert_eq!(c.hit((), now + wait), Ok(()));
        }
    }
}
