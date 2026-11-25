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
    pub class_name: Option<String>,
    pub subclass_id: Option<u32>,
    pub subclass_name: Option<String>,
    /// Copper a vendor pays for one.
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
    item_class: Option<Named>,
    #[serde(default)]
    item_subclass: Option<Named>,
    #[serde(default)]
    sell_price: Option<u64>,
}

#[derive(Deserialize)]
struct Typed {
    #[serde(rename = "type", default)]
    kind: String,
}

#[derive(Deserialize)]
struct Named {
    #[serde(default)]
    id: Option<u32>,
    #[serde(default)]
    name: Option<String>,
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
    let (class_id, class_name) = doc.item_class.map_or((None, None), |c| (c.id, c.name));
    let (subclass_id, subclass_name) = doc.item_subclass.map_or((None, None), |c| (c.id, c.name));
    Ok(Item {
        id: doc.id,
        name,
        quality: doc.quality.map_or_else(String::new, |q| q.kind),
        class_id,
        class_name,
        subclass_id,
        subclass_name,
        sell_price: doc.sell_price,
    })
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
                class_name: Some("Trade Goods".into()),
                subclass_id: Some(6),
                subclass_name: Some("Leather".into()),
                sell_price: Some(15),
            }
        );
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

    proptest! {
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
