//! What the pages show about a character, read from the Battle.net JSON documents we stored.
//!
//! Parsing is deliberately lenient: Forever's responses aren't documented, so every field is
//! optional and anything missing or oddly shaped is just not shown. Shapes are described in
//! `docs/battlenet-api.md`.

use serde_json::Value;

/// The character endpoints squadron fetches, in the order it fetches them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Endpoint {
    Summary,
    Media,
    Equipment,
    Specializations,
    Professions,
    /// Only fetched for hunters; everyone else gets a 404.
    HunterPets,
}

impl Endpoint {
    pub const ALL: [Self; 6] = [
        Self::Summary,
        Self::Media,
        Self::Equipment,
        Self::Specializations,
        Self::Professions,
        Self::HunterPets,
    ];

    /// The name stored in the database.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Summary => "summary",
            Self::Media => "media",
            Self::Equipment => "equipment",
            Self::Specializations => "specializations",
            Self::Professions => "professions",
            Self::HunterPets => "hunter-pets",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|e| e.as_str() == s)
    }

    /// Path segments after `/profile/wow/character/{name}-{surname}`.
    #[must_use]
    pub fn path(self) -> &'static [&'static str] {
        match self {
            Self::Summary => &[],
            Self::Media => &["character-media"],
            Self::Equipment => &["equipment"],
            Self::Specializations => &["specializations"],
            Self::Professions => &["professions"],
            Self::HunterPets => &["hunter-pets"],
        }
    }
}

/// The stored documents for one character; any may be missing.
#[derive(Debug, Default)]
pub struct Documents {
    pub summary: Option<Value>,
    pub media: Option<Value>,
    pub equipment: Option<Value>,
    pub specializations: Option<Value>,
    pub professions: Option<Value>,
    pub hunter_pets: Option<Value>,
}

impl Documents {
    pub fn set(&mut self, endpoint: Endpoint, doc: Value) {
        let slot = match endpoint {
            Endpoint::Summary => &mut self.summary,
            Endpoint::Media => &mut self.media,
            Endpoint::Equipment => &mut self.equipment,
            Endpoint::Specializations => &mut self.specializations,
            Endpoint::Professions => &mut self.professions,
            Endpoint::HunterPets => &mut self.hunter_pets,
        };
        *slot = Some(doc);
    }
}

#[derive(Debug, Default, PartialEq)]
pub struct Profile {
    /// The game's spelling of the name and surname.
    pub name: Option<String>,
    pub surname: Option<String>,
    pub level: Option<u64>,
    pub race: Option<String>,
    pub class: Option<String>,
    pub gender: Option<String>,
    /// `ALLIANCE` or `HORDE`.
    pub faction: Option<String>,
    pub guild: Option<String>,
    pub item_level: Option<u64>,
    /// Milliseconds since the epoch.
    pub last_login_ms: Option<i64>,
    pub avatar_url: Option<String>,
    /// Talent trees of the active group with points spent, most points first.
    pub talents: Vec<(String, u64)>,
    pub professions: Vec<Profession>,
    pub gear: Vec<Item>,
    /// Hunters' pets, the active one first.
    pub pets: Vec<Pet>,
}

#[derive(Debug, PartialEq)]
pub struct Pet {
    pub name: String,
    pub level: Option<u64>,
    /// The beast it was tamed from, e.g. `Plainstrider`.
    pub creature: Option<String>,
    pub active: bool,
}

#[derive(Debug, PartialEq)]
pub struct Profession {
    pub name: String,
    pub skill: u64,
    pub max: u64,
}

#[derive(Debug, PartialEq)]
pub struct Item {
    pub slot: String,
    pub name: String,
    /// Lower-case quality (`poor`, `common`, `uncommon`, `rare`, `epic`, `legendary`), for styling.
    pub quality: String,
    pub level: Option<u64>,
}

impl Profile {
    #[must_use]
    pub fn from_documents(docs: &Documents) -> Self {
        let mut p = Self::default();
        if let Some(s) = &docs.summary {
            p.name = str_at(s, &["name"]);
            p.surname = str_at(s, &["surname"]);
            p.level = s["level"].as_u64();
            p.race = str_at(s, &["race", "name"]);
            p.class = str_at(s, &["character_class", "name"]);
            p.gender = str_at(s, &["gender", "name"]);
            p.faction = str_at(s, &["faction", "type"]);
            p.guild = str_at(s, &["guild", "name"]);
            p.item_level = s["equipped_item_level"]
                .as_u64()
                .or_else(|| s["average_item_level"].as_u64());
            p.last_login_ms = s["last_login_timestamp"].as_i64();
        }
        p.avatar_url = docs.media.as_ref().and_then(avatar);
        p.talents = docs.specializations.as_ref().map_or_else(Vec::new, talents);
        p.professions = docs.professions.as_ref().map_or_else(Vec::new, professions);
        p.gear = docs.equipment.as_ref().map_or_else(Vec::new, gear);
        p.pets = docs.hunter_pets.as_ref().map_or_else(Vec::new, pets);
        p
    }

    /// Whether to fetch an endpoint, given the summary: pets only exist for hunters.
    #[must_use]
    pub fn wants(summary: &Value, endpoint: Endpoint) -> bool {
        endpoint != Endpoint::HunterPets
            || summary["character_class"]["id"].as_u64() == Some(HUNTER)
    }

    /// CSS-friendly class name (`Death Knight` → `deathknight`), for colouring.
    #[must_use]
    pub fn class_key(&self) -> String {
        self.class
            .as_deref()
            .unwrap_or_default()
            .chars()
            .filter(char::is_ascii_alphabetic)
            .map(|c| c.to_ascii_lowercase())
            .collect()
    }
}

/// Playable class id of hunters, the same in every WoW flavour.
const HUNTER: u64 = 3;

fn str_at(v: &Value, path: &[&str]) -> Option<String> {
    path.iter()
        .try_fold(v, |v, k| v.get(k))?
        .as_str()
        .map(str::to_owned)
}

fn avatar(media: &Value) -> Option<String> {
    let assets = media["assets"].as_array()?;
    ["avatar", "avatar-sd"].iter().find_map(|want| {
        assets
            .iter()
            .find(|a| a["key"].as_str() == Some(want))
            .and_then(|a| a["value"].as_str())
            .map(str::to_owned)
    })
}

fn talents(specs: &Value) -> Vec<(String, u64)> {
    let Some(groups) = specs["specialization_groups"].as_array() else {
        return Vec::new();
    };
    let active = groups
        .iter()
        .find(|g| g["is_active"].as_bool() == Some(true))
        .or_else(|| groups.first());
    let mut trees: Vec<(String, u64)> = active
        .and_then(|g| g["specializations"].as_array())
        .into_iter()
        .flatten()
        .filter_map(|s| {
            let name = s["specialization_name"]
                .as_str()
                .or_else(|| s["specialization"]["name"].as_str())?;
            Some((name.to_owned(), s["spent_points"].as_u64().unwrap_or(0)))
        })
        .collect();
    trees.sort_by_key(|t| std::cmp::Reverse(t.1));
    trees
}

fn professions(doc: &Value) -> Vec<Profession> {
    ["primaries", "secondaries"]
        .iter()
        .filter_map(|k| doc[k].as_array())
        .flatten()
        .filter_map(|p| {
            let name = p["profession"]["name"].as_str()?;
            let tier = p["tiers"].as_array()?.first()?;
            Some(Profession {
                name: name.to_owned(),
                skill: tier["skill_points"].as_u64()?,
                max: tier["max_skill_points"].as_u64()?,
            })
        })
        .collect()
}

fn gear(doc: &Value) -> Vec<Item> {
    doc["equipped_items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|i| {
            Some(Item {
                slot: str_at(i, &["slot", "name"])?,
                name: i["name"].as_str()?.to_owned(),
                quality: str_at(i, &["quality", "type"])
                    .unwrap_or_default()
                    .to_ascii_lowercase(),
                level: i["level"]["value"].as_u64(),
            })
        })
        .collect()
}

fn pets(doc: &Value) -> Vec<Pet> {
    let mut pets: Vec<Pet> = doc["hunter_pets"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| {
            Some(Pet {
                name: p["name"].as_str()?.to_owned(),
                level: p["level"].as_u64(),
                creature: str_at(p, &["creature", "name"]),
                active: p["is_active"].as_bool().unwrap_or(false),
            })
        })
        .collect();
    pets.sort_by_key(|p| !p.active);
    pets
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture(name: &str) -> Value {
        let path = format!("{}/fixtures/{name}.json", env!("CARGO_MANIFEST_DIR"));
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    fn full() -> Documents {
        let mut docs = Documents::default();
        for e in Endpoint::ALL {
            docs.set(e, fixture(e.as_str()));
        }
        docs
    }

    #[test]
    fn reads_the_summary() {
        let p = Profile::from_documents(&full());
        assert_eq!(p.name.as_deref(), Some("Aldric"));
        assert_eq!(p.surname.as_deref(), Some("Thornwood"));
        assert_eq!(p.level, Some(26));
        assert_eq!(p.race.as_deref(), Some("Night Elf"));
        assert_eq!(p.class.as_deref(), Some("Rogue"));
        assert_eq!(p.faction.as_deref(), Some("ALLIANCE"));
        assert_eq!(p.guild.as_deref(), Some("Order of the Lantern"));
        assert_eq!(p.item_level, Some(14));
        assert_eq!(p.last_login_ms, Some(1_794_955_893_000));
        assert_eq!(p.class_key(), "rogue");
    }

    #[test]
    fn reads_the_other_documents() {
        let p = Profile::from_documents(&full());
        assert_eq!(
            p.avatar_url.as_deref(),
            Some("https://render.example/forever-eu/character/1-avatar.jpg")
        );
        assert_eq!(
            p.talents,
            [("Combat".to_owned(), 17), ("Assassination".to_owned(), 5)]
        );
        assert_eq!(
            p.professions,
            [
                Profession {
                    name: "Skinning".into(),
                    skill: 158,
                    max: 225
                },
                Profession {
                    name: "Cooking".into(),
                    skill: 40,
                    max: 75
                },
            ]
        );
        assert_eq!(p.gear.len(), 2);
        assert_eq!(p.gear[0].quality, "poor");
        assert_eq!(p.gear[1].level, Some(18));
        assert_eq!(
            p.pets,
            [
                Pet {
                    name: "Biscuit".into(),
                    level: Some(24),
                    creature: Some("Plainstrider".into()),
                    active: true
                },
                Pet {
                    name: "Mossy".into(),
                    level: Some(22),
                    creature: Some("Forest Spider".into()),
                    active: false
                },
            ]
        );
    }

    #[test]
    fn pets_only_for_hunters() {
        let hunter = json!({"character_class": {"id": 3}});
        let rogue = fixture("summary");
        assert!(Profile::wants(&hunter, Endpoint::HunterPets));
        assert!(!Profile::wants(&rogue, Endpoint::HunterPets));
        assert!(Profile::wants(&rogue, Endpoint::Equipment));
    }

    #[test]
    fn tolerates_missing_and_odd_documents() {
        let mut docs = Documents::default();
        assert_eq!(Profile::from_documents(&docs), Profile::default());
        docs.set(Endpoint::Summary, json!({"level": "high", "race": []}));
        docs.set(Endpoint::Media, json!({"assets": "none"}));
        docs.set(
            Endpoint::Specializations,
            json!({"specialization_groups": [{}]}),
        );
        assert_eq!(Profile::from_documents(&docs), Profile::default());
    }

    #[test]
    fn falls_back_to_average_item_level() {
        let mut docs = Documents::default();
        docs.set(Endpoint::Summary, json!({"average_item_level": 9}));
        assert_eq!(Profile::from_documents(&docs).item_level, Some(9));
    }

    #[test]
    fn endpoint_names_round_trip() {
        for e in Endpoint::ALL {
            assert_eq!(Endpoint::parse(e.as_str()), Some(e));
        }
    }
}
