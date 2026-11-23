//! Guilds: their slugs, what the guild and roster documents say, and searching and paging the
//! roster for the guild page.
//!
//! A guild costs two requests (the guild and its roster), and the roster already has each
//! member's name, level, class and race, so members aren't looked up one by one. Battle.net
//! sends the whole roster at once, even for 800 members (see `docs/battlenet-api.md`). The
//! roster gives classes and races as ids only; [`class_name`] and [`race_name`] name them.

use std::fmt;

use serde_json::Value;

/// The guild documents zesquad fetches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Endpoint {
    Guild,
    Roster,
}

impl Endpoint {
    pub const ALL: [Self; 2] = [Self::Guild, Self::Roster];

    /// The name stored in the database.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Guild => "guild",
            Self::Roster => "roster",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|e| e.as_str() == s)
    }

    /// Path segments after `/data/wow/guild/{slug}`.
    #[must_use]
    pub fn path(self) -> &'static [&'static str] {
        match self {
            Self::Guild => &[],
            Self::Roster => &["roster"],
        }
    }
}

/// Longest slug we accept. Guild names are at most 24 characters in the game.
const MAX_SLUG: usize = 48;

/// A guild's slug, as Battle.net names it in paths: its name in lower case with dashes for
/// spaces (`order-of-the-lantern`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Slug(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not a guild")]
pub struct SlugError;

impl Slug {
    /// Takes a slug from a link: lower-cased, letters, digits and single dashes inside only.
    ///
    /// # Errors
    ///
    /// When it's empty, too long, or has anything else.
    pub fn parse(s: &str) -> Result<Self, SlugError> {
        let slug = s.to_lowercase();
        let ok = (1..=MAX_SLUG).contains(&slug.chars().count())
            && slug.chars().all(|c| c.is_alphanumeric() || c == '-')
            && !slug.starts_with('-')
            && !slug.ends_with('-')
            && !slug.contains("--");
        if ok { Ok(Self(slug)) } else { Err(SlugError) }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Slug {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// What the guild document says.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Guild {
    pub name: Option<String>,
    /// `ALLIANCE` or `HORDE`.
    pub faction: Option<String>,
    pub member_count: Option<u64>,
    pub achievement_points: Option<u64>,
    /// Milliseconds since the epoch.
    pub created_ms: Option<i64>,
}

impl Guild {
    #[must_use]
    pub fn parse(doc: &Value) -> Self {
        Self {
            name: doc["name"].as_str().map(str::to_owned),
            faction: doc["faction"]["type"].as_str().map(str::to_owned),
            member_count: doc["member_count"].as_u64(),
            achievement_points: doc["achievement_points"].as_u64(),
            created_ms: doc["created_timestamp"].as_i64(),
        }
    }
}

/// One member as the roster lists them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    pub name: String,
    pub surname: String,
    /// 0 is the guild master; higher is lower. Rank names aren't in the API.
    pub rank: Option<u64>,
    pub level: Option<u64>,
    pub class: Option<&'static str>,
    pub race: Option<&'static str>,
    /// `ALLIANCE` or `HORDE`.
    pub faction: Option<String>,
    /// `name surname` in lower case, for searching and sorting.
    search: String,
}

impl Member {
    /// CSS-friendly class name (`rogue`), the same as a class tag's key; empty when unknown.
    #[must_use]
    pub fn class_key(&self) -> String {
        class_key(self.class)
    }

    /// "Guild Master" for rank 0, else "Rank 3".
    #[must_use]
    pub fn rank_label(&self) -> String {
        match self.rank {
            Some(0) => "Guild Master".to_owned(),
            Some(n) => format!("Rank {n}"),
            None => String::new(),
        }
    }
}

fn class_key(class: Option<&str>) -> String {
    class
        .unwrap_or_default()
        .chars()
        .filter(char::is_ascii_alphabetic)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// The members a roster document lists, in its order. Entries without a name are left out.
#[must_use]
pub fn roster(doc: &Value) -> Vec<Member> {
    doc["members"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| {
            let c = &m["character"];
            let name = c["name"].as_str()?.to_owned();
            let surname = c["surname"].as_str().unwrap_or_default().to_owned();
            Some(Member {
                search: format!("{name} {surname}").to_lowercase(),
                rank: m["rank"].as_u64(),
                level: c["level"].as_u64(),
                class: c["playable_class"]["id"].as_u64().and_then(class_name),
                race: c["playable_race"]["id"].as_u64().and_then(race_name),
                faction: c["faction"]["type"].as_str().map(str::to_owned),
                name,
                surname,
            })
        })
        .collect()
}

/// Playable classes by id, from `/data/wow/playable-class/index` (`static-forever-us`,
/// 2026-11-23).
#[must_use]
pub fn class_name(id: u64) -> Option<&'static str> {
    Some(match id {
        1 => "Warrior",
        2 => "Paladin",
        3 => "Hunter",
        4 => "Rogue",
        5 => "Priest",
        7 => "Shaman",
        8 => "Mage",
        9 => "Warlock",
        11 => "Druid",
        _ => return None,
    })
}

/// Playable races by id, from `/data/wow/playable-race/index` (`static-forever-eu`).
#[must_use]
pub fn race_name(id: u64) -> Option<&'static str> {
    Some(match id {
        1 => "Human",
        2 => "Orc",
        3 => "Dwarf",
        4 => "Night Elf",
        5 => "Undead",
        6 => "Tauren",
        7 => "Gnome",
        8 => "Troll",
        95 => "High Order Skyborne",
        96 => "Windshaper Skyborne",
        _ => return None,
    })
}

/// How the guild roster is ordered.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Sort {
    /// Guild master first, then by rank, then highest level.
    #[default]
    Rank,
    Name,
    /// Highest first.
    Level,
}

impl Sort {
    pub const ALL: [Self; 3] = [Self::Rank, Self::Name, Self::Level];

    /// From a query; anything unknown is the default.
    #[must_use]
    pub fn parse(s: Option<&str>) -> Self {
        Self::ALL
            .into_iter()
            .find(|sort| Some(sort.as_str()) == s)
            .unwrap_or_default()
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rank => "rank",
            Self::Name => "name",
            Self::Level => "level",
        }
    }

    fn compare(self, a: &Member, b: &Member) -> std::cmp::Ordering {
        // Unknown ranks and levels go last.
        let rank = |m: &Member| m.rank.unwrap_or(u64::MAX);
        let level = |m: &Member| std::cmp::Reverse(m.level.map_or(0, |l| l + 1));
        let name = || a.search.cmp(&b.search);
        match self {
            Self::Rank => rank(a)
                .cmp(&rank(b))
                .then(level(a).cmp(&level(b)))
                .then_with(name),
            Self::Name => name(),
            Self::Level => level(a).cmp(&level(b)).then_with(name),
        }
    }
}

/// Members per page.
pub const PAGE_SIZE: usize = 50;

/// What the guild page asks of the roster: a name search, a class, an order and a page (from 1).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Query {
    /// Words that must each appear somewhere in `name surname`, ignoring case; empty matches
    /// everyone.
    pub search: String,
    /// A class key (`rogue`); `None` for every class.
    pub class: Option<String>,
    pub sort: Sort,
    pub page: usize,
}

impl Query {
    /// From the page's query string; anything odd falls back to its default.
    #[must_use]
    pub fn new(
        search: Option<&str>,
        class: Option<&str>,
        sort: Option<&str>,
        page: Option<&str>,
    ) -> Self {
        Self {
            search: search.unwrap_or_default().trim().to_lowercase(),
            class: class
                .map(|c| c.trim().to_lowercase())
                .filter(|c| !c.is_empty()),
            sort: Sort::parse(sort),
            page: page.and_then(|p| p.parse().ok()).unwrap_or(1).max(1),
        }
    }

    fn matches(&self, m: &Member) -> bool {
        self.search.split_whitespace().all(|w| m.search.contains(w))
            && self.class.as_deref().is_none_or(|c| m.class_key() == c)
    }
}

/// One page of the roster.
#[derive(Debug, PartialEq, Eq)]
pub struct Page<'a> {
    pub members: Vec<&'a Member>,
    /// Members that match, across every page.
    pub matched: usize,
    /// This page, from 1, after clamping to the last one.
    pub page: usize,
    /// At least 1, even with nothing matched.
    pub pages: usize,
}

/// The members matching `query`, sorted, and the page of them it asks for. A page past the end
/// shows the last one.
#[must_use]
pub fn page<'a>(members: &'a [Member], query: &Query) -> Page<'a> {
    let mut matched: Vec<&Member> = members.iter().filter(|m| query.matches(m)).collect();
    matched.sort_by(|a, b| query.sort.compare(a, b));
    let pages = matched.len().div_ceil(PAGE_SIZE).max(1);
    let page = query.page.clamp(1, pages);
    let members = matched
        .iter()
        .skip((page - 1) * PAGE_SIZE)
        .take(PAGE_SIZE)
        .copied()
        .collect();
    Page {
        members,
        matched: matched.len(),
        page,
        pages,
    }
}

/// The classes in the roster with how many of each, in the order of [`class_name`]'s ids.
#[must_use]
pub fn classes(members: &[Member]) -> Vec<(&'static str, usize)> {
    (1..=11)
        .filter_map(class_name)
        .map(|class| {
            let n = members.iter().filter(|m| m.class == Some(class)).count();
            (class, n)
        })
        .filter(|(_, n)| *n > 0)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use serde_json::json;

    fn fixture(name: &str) -> Value {
        let path = format!("{}/fixtures/{name}.json", env!("CARGO_MANIFEST_DIR"));
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    fn member(name: &str, rank: u64, level: u64, class: u64) -> Member {
        roster(&json!({"members": [{"rank": rank, "character": {
            "name": name, "surname": "Testwood", "level": level,
            "playable_class": {"id": class}, "playable_race": {"id": 1},
            "faction": {"type": "ALLIANCE"}
        }}]}))
        .remove(0)
    }

    #[test]
    fn reads_the_guild() {
        let g = Guild::parse(&fixture("guild"));
        assert_eq!(g.name.as_deref(), Some("Order of the Lantern"));
        assert_eq!(g.faction.as_deref(), Some("ALLIANCE"));
        assert_eq!(g.member_count, Some(61));
        assert_eq!(g.achievement_points, Some(0));
        assert_eq!(g.created_ms, Some(1_794_000_000_000));
    }

    #[test]
    fn reads_the_roster() {
        let members = roster(&fixture("guild-roster"));
        assert_eq!(members.len(), 61);
        let first = &members[0];
        assert_eq!(first.name, "Aldric");
        assert_eq!(first.surname, "Thornwood");
        assert_eq!(first.rank, Some(0));
        assert_eq!(first.rank_label(), "Guild Master");
        assert_eq!(first.level, Some(26));
        assert_eq!(first.class, Some("Rogue"));
        assert_eq!(first.class_key(), "rogue");
        assert_eq!(first.race, Some("Night Elf"));
        assert_eq!(first.faction.as_deref(), Some("ALLIANCE"));
    }

    #[test]
    fn tolerates_odd_documents() {
        assert_eq!(Guild::parse(&json!([])), Guild::default());
        assert_eq!(roster(&json!({"members": "none"})).len(), 0);
        let odd = roster(&json!({"members": [
            {"character": {"surname": "Nameless"}},
            {"rank": "x", "character": {"name": "Odd", "playable_class": {"id": 6}}}
        ]}));
        assert_eq!(odd.len(), 1);
        assert_eq!(odd[0].class, None);
        assert_eq!(odd[0].class_key(), "");
        assert_eq!(odd[0].rank_label(), "");
    }

    #[test]
    fn slugs() {
        assert_eq!(
            Slug::parse("Order-of-the-Lantern").unwrap().as_str(),
            "order-of-the-lantern"
        );
        assert!(Slug::parse("spark").is_ok());
        assert!(Slug::parse("éclat-d-or").is_ok());
        for bad in [
            "",
            "-x",
            "x-",
            "a--b",
            "a b",
            "a/b",
            "..",
            "a.b",
            &"a".repeat(49),
        ] {
            assert!(Slug::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn searches_filters_and_sorts() {
        let members = vec![
            member("Brannoc", 2, 60, 1),
            member("Aerith", 0, 40, 4),
            member("Corwin", 2, 60, 4),
            member("Dagny", 5, 12, 8),
        ];
        let names = |q: &Query| {
            page(&members, q)
                .members
                .iter()
                .map(|m| m.name.as_str())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            names(&Query::new(None, None, None, None)),
            ["Aerith", "Brannoc", "Corwin", "Dagny"]
        );
        assert_eq!(
            names(&Query::new(None, None, Some("level"), None)),
            ["Brannoc", "Corwin", "Aerith", "Dagny"]
        );
        assert_eq!(
            names(&Query::new(Some(" RW "), None, Some("name"), None)),
            ["Corwin"]
        );
        assert_eq!(
            names(&Query::new(Some("testwood"), None, None, None)).len(),
            4
        );
        // Every word has to appear, anywhere and in any order.
        assert_eq!(
            names(&Query::new(Some("test  dag"), None, None, None)),
            ["Dagny"]
        );
        assert_eq!(
            names(&Query::new(Some("dag cor"), None, None, None)).len(),
            0
        );
        assert_eq!(
            names(&Query::new(None, Some("Rogue"), None, None)),
            ["Aerith", "Corwin"]
        );
        assert_eq!(
            classes(&members),
            [("Warrior", 1), ("Rogue", 2), ("Mage", 1)]
        );
    }

    #[test]
    fn odd_queries_fall_back() {
        let q = Query::new(Some(""), Some(""), Some("best"), Some("-3"));
        assert_eq!(q, Query::new(None, None, None, None));
        assert_eq!(q.page, 1);
        assert_eq!(q.sort, Sort::Rank);
    }

    #[test]
    fn pages_past_the_end_show_the_last() {
        let members: Vec<Member> = (0..120)
            .map(|i| member(&format!("M{i:03}"), 1, 1, 1))
            .collect();
        let p = page(&members, &Query::new(None, None, None, Some("9")));
        assert_eq!(
            (p.page, p.pages, p.matched, p.members.len()),
            (3, 3, 120, 20)
        );
        let none = page(&members, &Query::new(Some("zzz"), None, None, Some("2")));
        assert_eq!((none.page, none.pages, none.matched), (1, 1, 0));
        assert_eq!(none.members.len(), 0);
    }

    #[test]
    fn endpoint_names_round_trip() {
        for e in Endpoint::ALL {
            assert_eq!(Endpoint::parse(e.as_str()), Some(e));
        }
    }

    fn arb_member() -> impl Strategy<Value = Member> {
        ("[A-Za-z]{2,8}", 0u64..8, 1u64..61, 0u64..13)
            .prop_map(|(name, rank, level, class)| member(&name, rank, level, class))
    }

    /// Whether `search` matches, worked out apart from `Query`.
    fn query_matches(search: &str, m: &Member) -> bool {
        let haystack = format!("{} {}", m.name, m.surname).to_lowercase();
        search.split(' ').all(|w| haystack.contains(w))
    }

    proptest! {
        #[test]
        fn every_match_is_on_exactly_one_page(
            members in prop::collection::vec(arb_member(), 0..180),
            search in "[a-z ]{0,4}",
            sort in prop::sample::select(Sort::ALL.to_vec()),
        ) {
            let q = |page| Query { search: search.clone(), class: None, sort, page };
            let first = page(&members, &q(1));
            let mut seen = 0;
            for n in 1..=first.pages {
                let p = page(&members, &q(n));
                prop_assert_eq!(p.page, n);
                prop_assert!(p.members.len() <= PAGE_SIZE);
                prop_assert!(p.members.iter().all(|m| query_matches(&search, m)));
                seen += p.members.len();
            }
            prop_assert_eq!(seen, first.matched);
            prop_assert!(first.matched <= members.len());
        }

        #[test]
        fn slugs_never_panic_and_stay_canonical(s in "\\PC{0,60}") {
            if let Ok(slug) = Slug::parse(&s) {
                prop_assert_eq!(Slug::parse(slug.as_str()), Ok(slug.clone()));
                prop_assert!(!slug.as_str().contains('/'));
            }
        }

        #[test]
        fn odd_rosters_never_panic(v in prop::collection::vec(any::<u8>(), 0..64)) {
            if let Ok(doc) = serde_json::from_slice::<Value>(&v) {
                let _ = roster(&doc);
                let _ = Guild::parse(&doc);
            }
        }
    }
}
