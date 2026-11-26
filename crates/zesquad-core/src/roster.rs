//! Ordering a squad's roster and the one-line summary above it.

use std::cmp::Ordering;

/// How the roster is ordered. `Added` (the order characters joined) is the default and is
/// left out of links.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Sort {
    #[default]
    Added,
    Name,
    Level,
    ItemLevel,
    LastPlayed,
}

impl Sort {
    pub const ALL: [Self; 5] = [
        Self::Added,
        Self::Name,
        Self::Level,
        Self::ItemLevel,
        Self::LastPlayed,
    ];

    /// From a query or form value; anything unknown is the default, so an old or mangled link
    /// still opens the squad.
    #[must_use]
    pub fn parse(s: Option<&str>) -> Self {
        Self::ALL
            .into_iter()
            .find(|sort| Some(sort.as_str()) == s)
            .unwrap_or_default()
    }

    /// The value in links.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Name => "name",
            Self::Level => "level",
            Self::ItemLevel => "item-level",
            Self::LastPlayed => "last-played",
        }
    }

    /// What the sort link says.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Name => "name",
            Self::Level => "level",
            Self::ItemLevel => "item level",
            Self::LastPlayed => "last played",
        }
    }

    /// Orders `a` before `b`. Numbers go highest (or most recent) first, and characters the
    /// Armory hasn't told us the number for go last. Item level ties, including characters
    /// without one, go by level, and level ties by item level. Then ties go by name, then by when they were added, so the
    /// order never depends on how the list came in.
    #[must_use]
    pub fn compare(self, a: &Entry, b: &Entry) -> Ordering {
        let by_name = || a.name.cmp(&b.name).then(a.added.cmp(&b.added));
        let highest_first = |x: Option<i64>, y: Option<i64>| match (x, y) {
            (Some(x), Some(y)) => y.cmp(&x),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        };
        match self {
            Self::Added => a.added.cmp(&b.added),
            Self::Name => by_name(),
            Self::Level => highest_first(a.level, b.level)
                .then(highest_first(a.item_level, b.item_level))
                .then_with(by_name),
            Self::ItemLevel => highest_first(a.item_level, b.item_level)
                .then(highest_first(a.level, b.level))
                .then_with(by_name),
            Self::LastPlayed => highest_first(a.last_played, b.last_played).then_with(by_name),
        }
    }
}

/// What a character is sorted by.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Entry {
    /// Its position in the squad by when it was added.
    pub added: usize,
    /// Lower case, for comparing.
    pub name: String,
    pub level: Option<i64>,
    pub item_level: Option<i64>,
    /// Unix milliseconds.
    pub last_played: Option<i64>,
}

/// What the summary counts, per character.
#[derive(Debug, Clone, Default)]
pub struct Counted {
    pub level: Option<u64>,
    /// As Battle.net says it: `ALLIANCE`, `HORDE`.
    pub faction: Option<String>,
}

/// "6 characters · 4 at level 60 · 3 Alliance, 3 Horde": how many, how many share the highest
/// level, and the factions. With a filter, `total` is the whole squad's count: "2 of 6
/// characters". Parts the Armory hasn't answered for are left out.
#[must_use]
pub fn summary(shown: &[Counted], total: usize) -> String {
    let n = shown.len();
    let plural = if total == 1 { "" } else { "s" };
    let mut parts = vec![if n == total {
        format!("{n} character{plural}")
    } else {
        format!("{n} of {total} character{plural}")
    }];
    if let Some(top) = shown.iter().filter_map(|c| c.level).max() {
        let at_top = shown.iter().filter(|c| c.level == Some(top)).count();
        parts.push(format!("{at_top} at level {top}"));
    }
    let count = |faction: &str| {
        shown
            .iter()
            .filter(|c| {
                c.faction
                    .as_deref()
                    .is_some_and(|f| f.eq_ignore_ascii_case(faction))
            })
            .count()
    };
    let factions: Vec<String> = [("ALLIANCE", "Alliance"), ("HORDE", "Horde")]
        .into_iter()
        .map(|(key, name)| (count(key), name))
        .filter(|(k, _)| *k > 0)
        .map(|(k, name)| format!("{k} {name}"))
        .collect();
    if !factions.is_empty() {
        parts.push(factions.join(", "));
    }
    parts.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn entry(added: usize, name: &str, level: Option<i64>) -> Entry {
        Entry {
            added,
            name: name.into(),
            level,
            ..Entry::default()
        }
    }

    fn sorted(sort: Sort, mut entries: Vec<Entry>) -> Vec<String> {
        entries.sort_by(|a, b| sort.compare(a, b));
        entries.into_iter().map(|e| e.name).collect()
    }

    #[test]
    fn parses_links() {
        for sort in Sort::ALL {
            assert_eq!(Sort::parse(Some(sort.as_str())), sort);
        }
        assert_eq!(Sort::parse(None), Sort::Added);
        assert_eq!(Sort::parse(Some("LEVEL")), Sort::Added);
        assert_eq!(Sort::parse(Some("<x>")), Sort::Added);
    }

    #[test]
    fn levels_go_highest_first_and_unknown_last() {
        let entries = vec![
            entry(0, "cora", Some(12)),
            entry(1, "abel", None),
            entry(2, "bram", Some(60)),
            entry(3, "aida", Some(12)),
        ];
        assert_eq!(
            sorted(Sort::Level, entries.clone()),
            ["bram", "aida", "cora", "abel"]
        );
        assert_eq!(
            sorted(Sort::Name, entries.clone()),
            ["abel", "aida", "bram", "cora"]
        );
        assert_eq!(
            sorted(Sort::Added, entries),
            ["cora", "abel", "bram", "aida"]
        );
    }

    #[test]
    fn level_and_item_level_break_each_others_ties() {
        let with = |added, name: &str, level, item_level| Entry {
            item_level,
            ..entry(added, name, level)
        };
        let entries = vec![
            with(0, "abel", Some(14), None),
            with(1, "bram", Some(60), None),
            with(2, "cora", Some(20), Some(18)),
            with(3, "dana", Some(30), Some(18)),
            with(4, "eda", None, None),
            with(5, "finn", Some(10), Some(25)),
        ];
        assert_eq!(
            sorted(Sort::ItemLevel, entries),
            ["finn", "dana", "cora", "bram", "abel", "eda"]
        );
        let entries = vec![
            with(0, "abel", Some(60), None),
            with(1, "bram", Some(60), Some(40)),
            with(2, "cora", Some(60), Some(55)),
        ];
        assert_eq!(sorted(Sort::Level, entries), ["cora", "bram", "abel"]);
    }

    #[test]
    fn summaries() {
        let c = |level, faction: &str| Counted {
            level,
            faction: Some(faction.into()),
        };
        let squad = [
            c(Some(60), "ALLIANCE"),
            c(Some(60), "HORDE"),
            c(Some(42), "ALLIANCE"),
        ];
        assert_eq!(
            summary(&squad, 3),
            "3 characters · 2 at level 60 · 2 Alliance, 1 Horde"
        );
        assert_eq!(
            summary(&squad[2..], 3),
            "1 of 3 characters · 1 at level 42 · 1 Alliance"
        );
        assert_eq!(summary(&[Counted::default()], 1), "1 character");
        assert_eq!(summary(&[], 0), "0 characters");
    }

    fn any_entry() -> impl Strategy<Value = Entry> {
        (
            0usize..50,
            "[a-c]{1,2}",
            prop::option::of(0i64..5),
            prop::option::of(0i64..5),
            prop::option::of(0i64..5),
        )
            .prop_map(|(added, name, level, item_level, last_played)| Entry {
                added,
                name,
                level,
                item_level,
                last_played,
            })
    }

    proptest! {
        /// A total order: sorting gives the same result whatever order the roster came in.
        #[test]
        fn sorting_ignores_the_order_it_came_in(
            mut entries in prop::collection::vec(any_entry(), 0..12),
            sort in prop::sample::select(Sort::ALL.to_vec()),
        ) {
            // Positions are unique in a real roster.
            for (i, e) in entries.iter_mut().enumerate() {
                e.added = i;
            }
            let mut forwards = entries.clone();
            forwards.sort_by(|a, b| sort.compare(a, b));
            let mut backwards: Vec<Entry> = entries.into_iter().rev().collect();
            backwards.sort_by(|a, b| sort.compare(a, b));
            prop_assert_eq!(forwards, backwards);
        }
    }
}
