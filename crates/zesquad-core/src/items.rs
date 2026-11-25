//! Item details (name, quality, class) from Battle.net's static game data, for the items seen at
//! the auction houses. Forever's own item data lacks many Classic items, so those are looked up
//! in Classic Era's. See "Items" in `docs/battlenet-api.md`.

use serde::Deserialize;

use crate::Region;

/// Item data is the same in every region; it's looked up in this one's, in its English.
pub const REGION: Region = Region::Eu;

/// Where an item's details came from, in the order they're tried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Forever's own item data.
    Forever,
    /// Classic Era's, for the Classic items Forever's doesn't have.
    Classic,
}

impl Source {
    pub const ALL: [Self; 2] = [Self::Forever, Self::Classic];

    /// As stored in the database.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Forever => "forever",
            Self::Classic => "classic",
        }
    }

    /// The static namespace to ask in `region`.
    #[must_use]
    pub fn namespace(self, region: Region) -> String {
        match self {
            Self::Forever => format!("static-forever-{}", region.as_str()),
            Self::Classic => format!("static-classic1x-{}", region.as_str()),
        }
    }
}

/// What the pages need of an item; the whole document is kept beside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub id: u32,
    pub name: String,
    /// `POOR`, `COMMON`, `UNCOMMON`, `RARE`, `EPIC`, ...; empty when missing.
    pub quality: String,
    pub class_id: Option<u32>,
    pub subclass_id: Option<u32>,
    /// Copper a vendor pays for one, as the tooltip says; `None` when it can't be sold.
    pub sell_price: Option<u64>,
}

#[derive(Debug, thiserror::Error)]
pub enum ItemError {
    #[error("item data isn't JSON in the expected shape: {0}")]
    Json(#[from] serde_json::Error),
    #[error("item data has no name")]
    NoName,
}

#[derive(Deserialize)]
struct Doc {
    id: u32,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    quality: Option<Typed>,
    #[serde(default)]
    item_class: Option<Class>,
    #[serde(default)]
    item_subclass: Option<Class>,
    #[serde(default)]
    sell_price: Option<u64>,
    #[serde(default)]
    preview_item: Option<Preview>,
}

/// The tooltip, as the game shows it.
#[derive(Deserialize)]
struct Preview {
    #[serde(default)]
    sell_price: Option<PreviewPrice>,
}

#[derive(Deserialize)]
struct PreviewPrice {
    #[serde(default)]
    value: Option<u64>,
}

#[derive(Deserialize)]
struct Typed {
    #[serde(rename = "type", default)]
    kind: String,
}

#[derive(Deserialize)]
struct Class {
    #[serde(default)]
    id: Option<u32>,
}

/// What a vendor pays: the tooltip's price, which is what the game shows. Forever repriced
/// some items without changing the top-level `sell_price` (which can even be 0), so that's only
/// used without a tooltip price. 0 means it can't be sold.
fn sell_price(preview: Option<PreviewPrice>, top: Option<u64>) -> Option<u64> {
    preview
        .and_then(|p| p.value)
        .or(top)
        .filter(|&copper| copper > 0)
}

/// Reads an item document. Leniently: only the id and a name are needed.
///
/// # Errors
///
/// When it isn't JSON with an id, or has no name.
pub fn parse(json: &str) -> Result<Item, ItemError> {
    let doc: Doc = serde_json::from_str(json)?;
    let name = doc
        .name
        .map(|n| n.trim().to_owned())
        .filter(|n| !n.is_empty())
        .ok_or(ItemError::NoName)?;
    let class_id = doc.item_class.and_then(|c| c.id);
    let subclass_id = doc.item_subclass.and_then(|c| c.id);
    Ok(Item {
        id: doc.id,
        name,
        quality: doc.quality.map_or_else(String::new, |q| q.kind),
        class_id,
        subclass_id,
        sell_price: sell_price(doc.preview_item.and_then(|p| p.sell_price), doc.sell_price),
    })
}

/// The game's own icon for an unknown item, a red question mark (`INV_Misc_QuestionMark`), for
/// items without one.
pub const UNKNOWN_ICON: u32 = 134_400;

#[derive(Deserialize)]
struct Media {
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    #[serde(default)]
    key: String,
    #[serde(default)]
    file_data_id: Option<u32>,
}

/// The icon's file id from an item's media document (`/data/wow/media/item/{id}`); `None`
/// when it has none.
///
/// # Errors
///
/// When it isn't JSON in the expected shape.
pub fn icon(media: &serde_json::Value) -> Result<Option<u32>, serde_json::Error> {
    let media = Media::deserialize(media)?;
    Ok(media
        .assets
        .into_iter()
        .find(|a| a.key == "icon")
        .and_then(|a| a.file_data_id))
}

/// Where Blizzard serves an icon, `size` pixels square (18, 36 or 56); the unknown item's
/// question mark for `None`. Forever's media names icons by file id.
#[must_use]
pub fn icon_url(icon: Option<u32>, size: u32) -> String {
    format!(
        "https://render.worldofwarcraft.com/forever-{}/icons/{size}/{}.jpg",
        REGION.as_str(),
        icon.unwrap_or(UNKNOWN_ICON)
    )
}

/// Longest search kept, in characters; item names are shorter.
pub const SEARCH_MAX: usize = 60;

/// What someone typed to search items by name, tidied like names (spaces trimmed and runs of
/// them made one, control characters dropped), at most [`SEARCH_MAX`] characters. `None` when
/// nothing's left.
#[must_use]
pub fn search(typed: &str) -> Option<String> {
    let tidy: String = crate::text::tidy(typed).chars().take(SEARCH_MAX).collect();
    let tidy = tidy.trim_end().to_owned();
    (!tidy.is_empty()).then_some(tidy)
}

/// A SQL `LIKE` pattern (with `ESCAPE '\\'`) matching names that contain `search` literally.
#[must_use]
pub fn contains_pattern(search: &str) -> String {
    format!("%{}%", escape_like(search))
}

/// A SQL `LIKE` pattern (with `ESCAPE '\\'`) matching names that start with `search`.
#[must_use]
pub fn prefix_pattern(search: &str) -> String {
    format!("{}%", escape_like(search))
}

fn escape_like(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Lower case, for the `q-<quality>` CSS classes.
#[must_use]
pub fn quality_class(quality: &str) -> String {
    quality.to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(format!(
            "{}/fixtures/{name}.json",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    }

    #[test]
    fn reads_an_item() {
        let item = parse(&fixture("item")).unwrap();
        assert_eq!(
            item,
            Item {
                id: 2318,
                name: "Light Leather".into(),
                quality: "COMMON".into(),
                class_id: Some(7),
                subclass_id: Some(6),
                sell_price: Some(15),
            }
        );
    }

    #[test]
    fn sell_price_is_the_tooltips() {
        let epic = parse(&fixture("item-epic")).unwrap();
        assert_eq!(epic.name, "Red Dragonscale Leggings");
        assert_eq!(epic.quality, "EPIC");
        assert_eq!(epic.sell_price, Some(42_088), "not the top-level 0");
        let price = |json: &str| parse(json).unwrap().sell_price;
        assert_eq!(
            price(
                r#"{"id": 1, "name": "a", "sell_price": 45267, "preview_item": {"sell_price": {"value": 47649}}}"#
            ),
            Some(47_649)
        );
        assert_eq!(
            price(r#"{"id": 1, "name": "a", "sell_price": 1, "preview_item": {}}"#),
            Some(1)
        );
        assert_eq!(
            price(r#"{"id": 1, "name": "a", "sell_price": 0}"#),
            None,
            "can't be sold"
        );
        assert_eq!(price(r#"{"id": 1, "name": "a"}"#), None);
    }

    #[test]
    fn needs_only_an_id_and_a_name() {
        let item = parse(r#"{"id": 5, "name": " Thing "}"#).unwrap();
        assert_eq!(item.name, "Thing");
        assert_eq!(item.quality, "");
        assert_eq!(item.class_id, None);
        assert!(matches!(parse(r#"{"id": 5}"#), Err(ItemError::NoName)));
        assert!(matches!(
            parse(r#"{"id": 5, "name": "  "}"#),
            Err(ItemError::NoName)
        ));
        assert!(matches!(parse(r#"{"name": "x"}"#), Err(ItemError::Json(_))));
    }

    #[test]
    fn sources_have_their_namespaces() {
        assert_eq!(Source::Forever.namespace(Region::Eu), "static-forever-eu");
        assert_eq!(Source::Classic.namespace(Region::Us), "static-classic1x-us");
        assert_eq!(Source::ALL[0], Source::Forever, "Forever's own first");
    }

    #[test]
    fn reads_an_icon() {
        let icon = |json: &str| icon(&serde_json::from_str(json).unwrap());
        assert_eq!(icon(&fixture("item-media")).unwrap(), Some(134_252));
        assert_eq!(icon(r#"{"assets": []}"#).unwrap(), None);
        assert_eq!(icon("{}").unwrap(), None);
        assert_eq!(
            icon(r#"{"assets": [{"key": "zoom", "file_data_id": 1}]}"#).unwrap(),
            None
        );
        assert!(icon(r#""icon""#).is_err());
    }

    #[test]
    fn icons_are_served_by_blizzard() {
        assert_eq!(
            icon_url(Some(134_252), 36),
            "https://render.worldofwarcraft.com/forever-eu/icons/36/134252.jpg"
        );
        assert_eq!(
            icon_url(None, 18),
            "https://render.worldofwarcraft.com/forever-eu/icons/18/134400.jpg"
        );
    }

    #[test]
    fn searches_are_tidied() {
        assert_eq!(search("  light   leather "), Some("light leather".into()));
        assert_eq!(search(" \t "), None);
        assert_eq!(search(&"x".repeat(100)).unwrap().len(), SEARCH_MAX);
    }

    #[test]
    fn like_patterns_match_literally() {
        assert_eq!(contains_pattern("leather"), "%leather%");
        assert_eq!(prefix_pattern("50%_off\\"), "50\\%\\_off\\\\%");
    }

    proptest! {
        #[test]
        fn tidy_searches_are_short_and_trimmed(s in ".*") {
            if let Some(t) = search(&s) {
                prop_assert!(t.chars().count() <= SEARCH_MAX);
                prop_assert_eq!(t.trim(), t.as_str());
                prop_assert!(!t.contains("  "));
            }
        }

        #[test]
        fn never_panics(s in ".*") {
            let _ = parse(&s);
        }

        #[test]
        fn keeps_any_name_trimmed(id in any::<u32>(), name in "[^\\x00-\\x1f\"\\\\]{0,40}") {
            let json = serde_json::json!({"id": id, "name": name}).to_string();
            match parse(&json) {
                Ok(item) => {
                    prop_assert_eq!(item.id, id);
                    prop_assert_eq!(item.name, name.trim());
                }
                Err(ItemError::NoName) => prop_assert!(name.trim().is_empty()),
                Err(e) => prop_assert!(false, "{e}"),
            }
        }
    }
}
