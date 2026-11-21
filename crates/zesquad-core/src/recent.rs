//! The squads a browser opened lately, kept in a cookie so the front page can list them.
//!
//! The cookie holds squad ids, newest first, separated by dots. It's read leniently: anything
//! that isn't shaped like a squad id is dropped, so a mangled cookie costs the list, never the
//! page. Ids are only ever ones this browser already opened, so the cookie reveals nothing new.

/// How many squads are remembered.
pub const MAX: usize = 10;

/// Separates ids in the cookie: not in either id form, and allowed in a cookie value.
const SEP: char = '.';

/// Squad ids, newest first, at most [`MAX`], no duplicates.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Recent(Vec<String>);

impl Recent {
    /// Reads a cookie value; ids not shaped like squad ids and duplicates are dropped.
    #[must_use]
    pub fn parse(cookie: &str) -> Self {
        let mut ids: Vec<String> = Vec::new();
        for id in cookie.split(SEP).filter(|id| looks_like_id(id)) {
            if ids.len() == MAX {
                break;
            }
            if !ids.iter().any(|seen| seen == id) {
                ids.push(id.to_owned());
            }
        }
        Self(ids)
    }

    /// Puts `id` first, keeping at most [`MAX`]. Ids not shaped like squad ids are ignored.
    pub fn visit(&mut self, id: &str) {
        if !looks_like_id(id) {
            return;
        }
        self.0.retain(|seen| seen != id);
        self.0.insert(0, id.to_owned());
        self.0.truncate(MAX);
    }

    pub fn forget(&mut self, id: &str) {
        self.0.retain(|seen| seen != id);
    }

    /// Keeps only the ids `keep` says yes to, e.g. squads that still exist.
    pub fn retain(&mut self, mut keep: impl FnMut(&str) -> bool) {
        self.0.retain(|id| keep(id));
    }

    #[must_use]
    pub fn ids(&self) -> &[String] {
        &self.0
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The cookie value.
    #[must_use]
    pub fn to_cookie(&self) -> String {
        self.0.join(&SEP.to_string())
    }
}

/// A squad id, new (26 base32 characters) or old (a dashed UUID, 36 characters): lower-case
/// letters, digits and dashes. Ids are looked up as plain strings, so this only keeps the
/// cookie tidy; it isn't what decides whether a squad exists.
fn looks_like_id(id: &str) -> bool {
    (26..=36).contains(&id.len())
        && id
            .bytes()
            .all(|c| c.is_ascii_digit() || c.is_ascii_lowercase() || c == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::squad_id;
    use proptest::prelude::*;

    const A: &str = "1z9k4m7q2x8v5n3b6c0d1e7f8g";
    const B: &str = "00000000000000000000000001";
    const OLD: &str = "3f2b9c1e-8a4d-4e7f-9b21-6c0d5a7e8f13";

    #[test]
    fn reads_and_writes_the_cookie() {
        let recent = Recent::parse(&format!("{A}.{OLD}"));
        assert_eq!(recent.ids(), [A, OLD]);
        assert_eq!(recent.to_cookie(), format!("{A}.{OLD}"));
        assert!(Recent::parse("").is_empty());
    }

    #[test]
    fn drops_what_isnt_an_id() {
        let recent = Recent::parse(&format!("..{A}.short.{}.<script>.{A}", A.to_uppercase()));
        assert_eq!(recent.ids(), [A]);
    }

    #[test]
    fn visiting_moves_to_the_front() {
        let mut recent = Recent::parse(&format!("{A}.{B}"));
        recent.visit(B);
        assert_eq!(recent.ids(), [B, A]);
        recent.visit("nope");
        assert_eq!(recent.ids(), [B, A]);
        recent.forget(B);
        assert_eq!(recent.ids(), [A]);
    }

    #[test]
    fn keeps_the_newest_ten() {
        let mut recent = Recent::default();
        for n in 0..15u128 {
            recent.visit(&squad_id::encode(n));
        }
        assert_eq!(recent.ids().len(), MAX);
        assert_eq!(recent.ids()[0], squad_id::encode(14));
        assert_eq!(recent.ids()[MAX - 1], squad_id::encode(5));
    }

    proptest! {
        #[test]
        fn any_cookie_reads_to_a_short_list_of_distinct_ids(cookie in ".{0,600}") {
            let recent = Recent::parse(&cookie);
            prop_assert!(recent.ids().len() <= MAX);
            let mut sorted = recent.ids().to_vec();
            sorted.sort();
            sorted.dedup();
            prop_assert_eq!(sorted.len(), recent.ids().len());
            prop_assert!(recent.ids().iter().all(|id| looks_like_id(id)));
            // What's written reads back the same.
            prop_assert_eq!(Recent::parse(&recent.to_cookie()), recent);
        }

        #[test]
        fn a_visited_squad_comes_first(
            ids in prop::collection::vec(any::<u128>(), 0..20),
            last in any::<u128>(),
        ) {
            let mut recent = Recent::default();
            for id in &ids {
                recent.visit(&squad_id::encode(*id));
            }
            let last = squad_id::encode(last);
            recent.visit(&last);
            prop_assert_eq!(&recent.ids()[0], &last);
            prop_assert!(recent.ids().len() <= MAX);
            prop_assert_eq!(Recent::parse(&recent.to_cookie()), recent);
        }
    }
}
