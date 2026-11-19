//! Tags a character gets from its Armory data, and tag colours.
//!
//! Automatic tags (class, main spec, race, faction, professions) are worked out from the parsed
//! profile every time, never stored, so they follow the character and can't be removed.

use std::fmt;

use crate::Tag;
use crate::armory::Profile;

/// A tag on a character: typed in by someone, or from the Armory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CharacterTag {
    pub label: String,
    /// Lower case, for comparing.
    pub key: String,
    /// From the Armory: can't be removed.
    pub auto: bool,
}

/// The tags a character's Armory data gives it: class, main spec, race, faction, then
/// professions as the Armory lists them. Repeats and names too long for a tag are left out.
#[must_use]
pub fn auto_tags(profile: &Profile) -> Vec<Tag> {
    let faction = profile.faction.as_deref().map(title_case);
    let candidates = [
        profile.class.as_deref(),
        main_spec(&profile.talents),
        profile.race.as_deref(),
        faction.as_deref(),
    ]
    .into_iter()
    .flatten()
    .chain(profile.professions.iter().map(|p| p.name.as_str()));
    let mut tags: Vec<Tag> = Vec::new();
    for name in candidates {
        if let Ok(Some(tag)) = Tag::new(name)
            && !tags.contains(&tag)
        {
            tags.push(tag);
        }
    }
    tags
}

/// The talent tree with the most points, if there's a clear one: `None` with no points spent
/// or when two trees tie for the most.
#[must_use]
fn main_spec(talents: &[(String, u64)]) -> Option<&str> {
    let most = talents.iter().map(|(_, points)| *points).max()?;
    let mut top = talents.iter().filter(|(_, points)| *points == most);
    let (tree, _) = top.next()?;
    (most > 0 && top.next().is_none()).then_some(tree.as_str())
}

/// `ALLIANCE` → `Alliance`.
fn title_case(s: &str) -> String {
    let lower = s.to_lowercase();
    let mut chars = lower.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// A character's tags: the automatic ones first, then the typed ones (`(label, key)`) in the
/// order given. A typed tag the Armory also gives is shown once, as automatic.
#[must_use]
pub fn merge(auto: Vec<Tag>, manual: Vec<(String, String)>) -> Vec<CharacterTag> {
    let mut tags: Vec<CharacterTag> = auto
        .into_iter()
        .map(|t| CharacterTag {
            label: t.label().to_owned(),
            key: t.key().to_owned(),
            auto: true,
        })
        .collect();
    for (label, key) in manual {
        if !tags.iter().any(|t| t.key == key) {
            tags.push(CharacterTag {
                label,
                key,
                auto: false,
            });
        }
    }
    tags
}

/// A colour as `#rrggbb`, lower case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Colour(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("pick a colour like #3fc7eb")]
pub struct ColourError;

impl Colour {
    /// # Errors
    ///
    /// Unless the input is `#` and six hex digits, which is what `<input type="color">` sends.
    pub fn parse(s: &str) -> Result<Self, ColourError> {
        let s = s.trim();
        match s.strip_prefix('#') {
            Some(hex) if hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit()) => {
                Ok(Self(s.to_ascii_lowercase()))
            }
            _ => Err(ColourError),
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Black or white, whichever is easier to read on this colour (WCAG contrast).
    #[must_use]
    pub fn text(&self) -> &'static str {
        let l = self.luminance();
        // Contrast with black is (l + 0.05) / 0.05, with white 1.05 / (l + 0.05).
        if (l + 0.05) * (l + 0.05) >= 0.05 * 1.05 {
            "#000000"
        } else {
            "#ffffff"
        }
    }

    /// WCAG relative luminance, 0 (black) to 1 (white).
    fn luminance(&self) -> f64 {
        let channel = |i: usize| {
            let v = f64::from(u8::from_str_radix(&self.0[i..i + 2], 16).unwrap_or(0)) / 255.0;
            if v <= 0.040_45 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(1) + 0.7152 * channel(3) + 0.0722 * channel(5)
    }
}

impl fmt::Display for Colour {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The classic client's class colours (`RAID_CLASS_COLORS`), for tags named after a class.
const CLASS_COLOURS: [(&str, &str); 9] = [
    ("warrior", "#c79c6e"),
    ("paladin", "#f58cba"),
    ("hunter", "#abd473"),
    ("rogue", "#fff569"),
    ("priest", "#ffffff"),
    ("shaman", "#0070de"),
    ("mage", "#69ccf0"),
    ("warlock", "#9482c9"),
    ("druid", "#ff7d0a"),
];

/// The colour a tag has until someone picks one: its class colour, if it's a class.
#[must_use]
pub fn default_colour(tag_key: &str) -> Option<Colour> {
    CLASS_COLOURS
        .iter()
        .find(|(class, _)| *class == tag_key)
        .map(|(_, hex)| Colour((*hex).to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::armory::Profession;
    use proptest::prelude::*;

    fn labels(tags: &[Tag]) -> Vec<&str> {
        tags.iter().map(Tag::label).collect()
    }

    fn profession(name: &str) -> Profession {
        Profession {
            name: name.to_owned(),
            skill: 1,
            max: 75,
        }
    }

    #[test]
    fn auto_tags_cover_the_basics_in_order() {
        let profile = Profile {
            class: Some("Rogue".into()),
            race: Some("Night Elf".into()),
            faction: Some("ALLIANCE".into()),
            talents: vec![("Combat".into(), 17), ("Assassination".into(), 5)],
            professions: vec![profession("Skinning"), profession("Cooking")],
            ..Profile::default()
        };
        assert_eq!(
            labels(&auto_tags(&profile)),
            [
                "Rogue",
                "Combat",
                "Night Elf",
                "Alliance",
                "Skinning",
                "Cooking"
            ]
        );
    }

    #[test]
    fn no_armory_data_no_tags() {
        assert_eq!(auto_tags(&Profile::default()), []);
    }

    #[test]
    fn auto_tags_skip_repeats_and_overlong_names() {
        let profile = Profile {
            class: Some("Mage".into()),
            professions: vec![profession("mage"), profession(&"x".repeat(40))],
            ..Profile::default()
        };
        assert_eq!(labels(&auto_tags(&profile)), ["Mage"]);
    }

    #[test]
    fn main_spec_needs_a_clear_winner() {
        let t = |v: &[(&str, u64)]| -> Vec<(String, u64)> {
            v.iter().map(|(n, p)| ((*n).to_owned(), *p)).collect()
        };
        assert_eq!(main_spec(&t(&[("Fire", 31), ("Arcane", 20)])), Some("Fire"));
        assert_eq!(main_spec(&t(&[("Arcane", 5), ("Fire", 11)])), Some("Fire"));
        assert_eq!(main_spec(&t(&[("Fire", 10), ("Frost", 10)])), None);
        assert_eq!(main_spec(&t(&[("Fire", 0), ("Frost", 0)])), None);
        assert_eq!(main_spec(&[]), None);
    }

    #[test]
    fn merge_puts_auto_first_and_shows_overlaps_once() {
        let auto = vec![
            Tag::new("Mage").unwrap().unwrap(),
            Tag::new("Fire").unwrap().unwrap(),
        ];
        let manual = vec![
            ("mage".to_owned(), "mage".to_owned()),
            ("Raid".to_owned(), "raid".to_owned()),
        ];
        let merged = merge(auto, manual);
        let shown: Vec<_> = merged.iter().map(|t| (t.label.as_str(), t.auto)).collect();
        assert_eq!(shown, [("Mage", true), ("Fire", true), ("Raid", false)]);
    }

    #[test]
    fn colours_parse_like_colour_inputs_send_them() {
        assert_eq!(Colour::parse("#3FC7EB").unwrap().as_str(), "#3fc7eb");
        for bad in [
            "",
            "3fc7eb",
            "#3fc7e",
            "#3fc7ebb",
            "#ggggg0",
            "red",
            "#3fc7eb;x",
        ] {
            assert_eq!(Colour::parse(bad), Err(ColourError), "{bad}");
        }
    }

    #[test]
    fn text_stays_readable() {
        let text = |hex: &str| Colour::parse(hex).unwrap().text();
        assert_eq!(text("#ffffff"), "#000000");
        assert_eq!(text("#fff569"), "#000000"); // rogue
        assert_eq!(text("#000000"), "#ffffff");
        assert_eq!(text("#0070de"), "#ffffff"); // shaman
    }

    #[test]
    fn every_class_has_a_colour() {
        for class in [
            "warrior", "paladin", "hunter", "rogue", "priest", "shaman", "mage", "warlock", "druid",
        ] {
            assert!(default_colour(class).is_some(), "{class}");
        }
        assert_eq!(default_colour("raid"), None);
    }

    proptest! {
        #[test]
        fn chosen_text_contrasts_at_least_as_well(r: u8, g: u8, b: u8) {
            let c = Colour::parse(&format!("#{r:02x}{g:02x}{b:02x}")).unwrap();
            let l = c.luminance();
            let black = (l + 0.05) / 0.05;
            let white = 1.05 / (l + 0.05);
            let (chosen, other) = if c.text() == "#000000" { (black, white) } else { (white, black) };
            prop_assert!(chosen >= other);
        }

        #[test]
        fn merged_tags_have_unique_keys_and_keep_every_auto_tag(
            auto in prop::collection::vec("[a-c]{1,2}", 0..6),
            manual in prop::collection::vec("[a-c]{1,2}", 0..6),
        ) {
            let auto = crate::parse_tags(&auto.join(",")).unwrap();
            let manual: Vec<_> = crate::parse_tags(&manual.join(","))
                .unwrap()
                .into_iter()
                .map(|t| (t.label().to_owned(), t.key().to_owned()))
                .collect();
            let merged = merge(auto.clone(), manual.clone());
            let mut keys: Vec<_> = merged.iter().map(|t| t.key.as_str()).collect();
            keys.sort_unstable();
            keys.dedup();
            prop_assert_eq!(keys.len(), merged.len());
            for t in &auto {
                prop_assert!(merged.iter().any(|m| m.key == t.key() && m.auto));
            }
            for (_, key) in &manual {
                prop_assert!(merged.iter().any(|m| &m.key == key));
            }
        }
    }
}
