//! Battle.net regions WoW: Forever runs in. Characters exist in one region only, so each squad
//! has one, and its characters are looked up there.

use std::fmt;
use std::str::FromStr;

/// A region with Forever characters. KR answers 403 for Forever, so it isn't offered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Region {
    Eu,
    Us,
    Tw,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown region {0:?}: use eu, us or tw")]
pub struct RegionError(String);

impl Region {
    pub const ALL: [Self; 3] = [Self::Eu, Self::Us, Self::Tw];

    /// The API's name for it, as stored in the database: `eu`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Eu => "eu",
            Self::Us => "us",
            Self::Tw => "tw",
        }
    }

    /// For people: `Europe`.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Eu => "Europe",
            Self::Us => "Americas",
            Self::Tw => "Taiwan",
        }
    }

    /// Battle.net answers in this language: the region's English.
    #[must_use]
    pub fn locale(self) -> &'static str {
        match self {
            Self::Eu => "en_GB",
            Self::Us | Self::Tw => "en_US",
        }
    }

    /// The namespace for Forever characters here.
    #[must_use]
    pub fn profile_namespace(self) -> String {
        format!("profile-forever-{}", self.as_str())
    }

    /// The namespace for Forever's changing game data here, such as auctions.
    #[must_use]
    pub fn dynamic_namespace(self) -> String {
        format!("dynamic-forever-{}", self.as_str())
    }
}

impl FromStr for Region {
    type Err = RegionError;

    /// Accepts any case and surrounding spaces.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let wanted = s.trim();
        Self::ALL
            .into_iter()
            .find(|r| r.as_str().eq_ignore_ascii_case(wanted))
            .ok_or_else(|| RegionError(s.to_owned()))
    }
}

/// For reading it from the database.
impl TryFrom<String> for Region {
    type Error = RegionError;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        s.parse()
    }
}

impl fmt::Display for Region {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn names_round_trip() {
        for region in Region::ALL {
            assert_eq!(region.as_str().parse(), Ok(region));
            assert_eq!(region.to_string(), region.as_str());
        }
    }

    #[test]
    fn parsing_ignores_case_and_spaces() {
        assert_eq!(" US ".parse(), Ok(Region::Us));
        assert_eq!("Eu".parse(), Ok(Region::Eu));
    }

    #[test]
    fn rejects_other_regions() {
        assert!("kr".parse::<Region>().is_err());
        assert!("".parse::<Region>().is_err());
        assert!("europe".parse::<Region>().is_err());
    }

    #[test]
    fn namespaces_and_locales() {
        assert_eq!(Region::Eu.profile_namespace(), "profile-forever-eu");
        assert_eq!(Region::Tw.profile_namespace(), "profile-forever-tw");
        assert_eq!(Region::Us.dynamic_namespace(), "dynamic-forever-us");
        assert_eq!(Region::Eu.locale(), "en_GB");
        assert_eq!(Region::Us.locale(), "en_US");
    }

    proptest! {
        #[test]
        fn never_panics(s in ".*") {
            let _ = s.parse::<Region>();
        }
    }
}
