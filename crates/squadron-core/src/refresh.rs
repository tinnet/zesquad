//! When a character's Armory data is due for another fetch.

/// Where a character's Armory data stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Never fetched, or a refresh was asked for.
    Pending,
    /// Fetched; the data is shown.
    Found,
    /// The API answered 404: no such character, or Forever's lookup isn't routed yet.
    NotFound,
    /// The last fetch failed (network, 5xx, bad credentials).
    Failed,
}

impl Status {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Found => "found",
            Self::NotFound => "not_found",
            Self::Failed => "failed",
        }
    }

    /// Unknown strings count as pending, so a bad row gets fetched again rather than stuck.
    #[must_use]
    pub fn parse(s: &str) -> Self {
        match s {
            "found" => Self::Found,
            "not_found" => Self::NotFound,
            "failed" => Self::Failed,
            _ => Self::Pending,
        }
    }
}

/// Whether to fetch again. Character data only changes when a character logs out, so found
/// characters wait the full interval. Failures retry sooner (a tenth of it) because they're
/// usually brief outages.
#[must_use]
pub fn is_due(status: Status, fetched_at: Option<i64>, now: i64, interval_secs: i64) -> bool {
    let Some(fetched_at) = fetched_at else {
        return true;
    };
    let wait = match status {
        Status::Pending => 0,
        Status::Failed => interval_secs / 10,
        Status::Found | Status::NotFound => interval_secs,
    };
    now - fetched_at >= wait
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_round_trip() {
        for s in [
            Status::Pending,
            Status::Found,
            Status::NotFound,
            Status::Failed,
        ] {
            assert_eq!(Status::parse(s.as_str()), s);
        }
        assert_eq!(Status::parse("nonsense"), Status::Pending);
    }

    #[test]
    fn due_times() {
        let hour = 3600;
        assert!(is_due(Status::Found, None, 0, hour));
        assert!(is_due(Status::Pending, Some(100), 100, hour));
        assert!(!is_due(Status::Found, Some(0), hour - 1, hour));
        assert!(is_due(Status::Found, Some(0), hour, hour));
        assert!(!is_due(Status::Failed, Some(0), 359, hour));
        assert!(is_due(Status::Failed, Some(0), 360, hour));
    }
}
