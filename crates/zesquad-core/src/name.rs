//! Forever character names: a first name and a surname, no realm.

use std::fmt;
use std::str::FromStr;

/// Longest first name or surname we accept. The game allows 12 letters for a first name;
/// Forever's surname limit isn't documented, so this is generous rather than exact.
const MAX_PART: usize = 24;
const MIN_PART: usize = 2;

/// A WoW: Forever character, `Name-Surname`, as the Battle.net API identifies it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CharacterName {
    name: String,
    surname: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NameError {
    #[error("enter the character as Name-Surname")]
    Shape,
    #[error("names and surnames are {MIN_PART} to {MAX_PART} letters")]
    Length,
    #[error("names and surnames can only contain letters")]
    Letters,
}

impl CharacterName {
    /// Builds a name from its two parts, normalising them the way the game shows them.
    ///
    /// # Errors
    ///
    /// When either part is too short, too long or has anything but letters.
    pub fn new(name: &str, surname: &str) -> Result<Self, NameError> {
        Ok(Self {
            name: normalise(name, true)?,
            surname: normalise(surname, false)?,
        })
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn surname(&self) -> &str {
        &self.surname
    }

    /// Lower-case `name-surname`: equal for two spellings of the same character, and the
    /// path segment the Battle.net API expects.
    #[must_use]
    pub fn key(&self) -> String {
        format!("{}-{}", self.name, self.surname).to_lowercase()
    }
}

/// The game capitalises the first letter of a name and lower-cases the rest. Surnames keep
/// inner capitals people type (`McIntyre`) because we don't know Forever's rule yet, unless
/// the whole thing is in capitals. The key is case-insensitive either way, and once the
/// Armory knows the character, pages show its spelling.
fn normalise(part: &str, lower_rest: bool) -> Result<String, NameError> {
    let part = part.trim();
    let len = part.chars().count();
    if !(MIN_PART..=MAX_PART).contains(&len) {
        return Err(NameError::Length);
    }
    if !part.chars().all(char::is_alphabetic) {
        return Err(NameError::Letters);
    }
    // Judged on the letters after the first, so normalising twice changes nothing.
    let lower_rest = lower_rest || part.chars().skip(1).all(char::is_uppercase);
    let mut chars = part.chars();
    let first = chars.next().into_iter().flat_map(char::to_uppercase);
    let rest: String = if lower_rest {
        chars.flat_map(char::to_lowercase).collect()
    } else {
        chars.collect()
    };
    Ok(first.chain(rest.chars()).collect())
}

/// Accepts `Name-Surname` and `Name Surname`, the two ways people write them.
impl FromStr for CharacterName {
    type Err = NameError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        let (name, surname) = s
            .split_once('-')
            .or_else(|| s.split_once(char::is_whitespace))
            .ok_or(NameError::Shape)?;
        Self::new(name, surname)
    }
}

impl fmt::Display for CharacterName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", self.name, self.surname)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn normalises_capitals() {
        let c: CharacterName = "  aRTHAS-menethil ".parse().unwrap();
        assert_eq!(c.to_string(), "Arthas-Menethil");
        assert_eq!(c.key(), "arthas-menethil");
    }

    #[test]
    fn keeps_inner_capitals_in_surnames() {
        let c: CharacterName = "flint-McIntyre".parse().unwrap();
        assert_eq!(c.surname(), "McIntyre");
        let shouted: CharacterName = "FLINT-MCINTYRE".parse().unwrap();
        assert_eq!(shouted.to_string(), "Flint-Mcintyre");
    }

    #[test]
    fn accepts_a_space_and_accents() {
        let c: CharacterName = "Zoë Ångström".parse().unwrap();
        assert_eq!(c.to_string(), "Zoë-Ångström");
    }

    #[test]
    fn rejects_bad_input() {
        assert_eq!("Arthas".parse::<CharacterName>(), Err(NameError::Shape));
        assert_eq!(
            "A-Menethil".parse::<CharacterName>(),
            Err(NameError::Length)
        );
        assert_eq!(
            "Arth4s-Menethil".parse::<CharacterName>(),
            Err(NameError::Letters)
        );
        assert_eq!(
            "Arthas-Mene-thil".parse::<CharacterName>(),
            Err(NameError::Letters)
        );
    }

    proptest! {
        #[test]
        fn display_round_trips(name in "[a-zA-Zäöü]{2,12}", surname in "[a-zA-Z]{2,20}") {
            let c = CharacterName::new(&name, &surname).unwrap();
            let again: CharacterName = c.to_string().parse().unwrap();
            prop_assert_eq!(&again, &c);
        }

        #[test]
        fn key_ignores_case(name in "[a-z]{2,12}", surname in "[a-z]{2,20}") {
            let lower = CharacterName::new(&name, &surname).unwrap();
            let upper = CharacterName::new(&name.to_uppercase(), &surname.to_uppercase()).unwrap();
            prop_assert_eq!(lower.key(), upper.key());
        }

        #[test]
        fn never_panics(s in "\\PC*") {
            let _ = s.parse::<CharacterName>();
        }
    }
}
