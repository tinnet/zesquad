//! Free text people type: squad names and tags.

use std::fmt;

const MAX_SQUAD_NAME: usize = 64;
const MAX_TAG: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TextError {
    #[error("give the squad a name")]
    EmptyName,
    #[error("squad names are at most {MAX_SQUAD_NAME} characters")]
    LongName,
    #[error("tags are at most {MAX_TAG} characters")]
    LongTag,
}

/// Trims and collapses runs of whitespace, and drops control characters, so that names that
/// look the same are the same.
pub(crate) fn tidy(s: &str) -> String {
    s.split(|c: char| c.is_whitespace() || c.is_control())
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// A squad's display name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SquadName(String);

impl SquadName {
    /// # Errors
    ///
    /// When the name is empty after tidying, or too long.
    pub fn new(s: &str) -> Result<Self, TextError> {
        let s = tidy(s);
        match s.chars().count() {
            0 => Err(TextError::EmptyName),
            n if n > MAX_SQUAD_NAME => Err(TextError::LongName),
            _ => Ok(Self(s)),
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A free-form label on a character. Tags compare case-insensitively (`Raid` = `raid`) but
/// keep the spelling they were first entered with.
#[derive(Debug, Clone)]
pub struct Tag {
    label: String,
    key: String,
}

impl Tag {
    /// `None` for input that is empty after tidying.
    ///
    /// # Errors
    ///
    /// When the tag is too long.
    pub fn new(s: &str) -> Result<Option<Self>, TextError> {
        let label = tidy(s);
        match label.chars().count() {
            0 => Ok(None),
            n if n > MAX_TAG => Err(TextError::LongTag),
            _ => Ok(Some(Self {
                key: label.to_lowercase(),
                label,
            })),
        }
    }

    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    #[must_use]
    pub fn key(&self) -> &str {
        &self.key
    }
}

impl PartialEq for Tag {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}

impl Eq for Tag {}

impl fmt::Display for Tag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.label)
    }
}

/// Splits a comma-separated tag list, dropping blanks and repeats.
///
/// # Errors
///
/// When any tag is too long.
pub fn parse_tags(s: &str) -> Result<Vec<Tag>, TextError> {
    let mut tags: Vec<Tag> = Vec::new();
    for part in s.split(',') {
        if let Some(tag) = Tag::new(part)?
            && !tags.contains(&tag)
        {
            tags.push(tag);
        }
    }
    Ok(tags)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn squad_names_are_tidied() {
        assert_eq!(
            SquadName::new("  Thursday \t raiders ").unwrap().as_str(),
            "Thursday raiders"
        );
        assert_eq!(SquadName::new(" \n "), Err(TextError::EmptyName));
        assert_eq!(SquadName::new(&"x".repeat(65)), Err(TextError::LongName));
    }

    #[test]
    fn tags_split_on_commas_and_dedupe_ignoring_case() {
        let tags = parse_tags(" Raid, alts ,, raid , Main  tank").unwrap();
        let labels: Vec<_> = tags.iter().map(Tag::label).collect();
        assert_eq!(labels, ["Raid", "alts", "Main tank"]);
        assert_eq!(tags[0].key(), "raid");
    }

    #[test]
    fn long_tags_are_rejected() {
        assert_eq!(parse_tags(&"x".repeat(33)), Err(TextError::LongTag));
    }

    proptest! {
        #[test]
        fn tidy_is_idempotent(s in "\\PC*") {
            let once = tidy(&s);
            prop_assert_eq!(tidy(&once), once.clone());
            prop_assert!(!once.starts_with(' ') && !once.ends_with(' '));
        }

        #[test]
        fn parsed_tags_have_unique_keys(s in "[a-zA-Z ,]{0,80}") {
            // Inputs with a tag over the length limit are rejected, which another test covers.
            let Ok(tags) = parse_tags(&s) else { return Ok(()) };
            let mut keys: Vec<_> = tags.iter().map(Tag::key).collect();
            keys.sort_unstable();
            keys.dedup();
            prop_assert_eq!(keys.len(), tags.len());
        }
    }
}
