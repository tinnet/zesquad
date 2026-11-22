//! Logging in with Battle.net: who Battle.net says someone is, and the short-lived cookie that
//! carries a login's `state` and where to return to across the trip to Battle.net.

use std::fmt::Write as _;

use serde_json::Value;

use crate::redirect::safe_next;

/// Who someone is, from Battle.net's `/userinfo`: all zesquad asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    /// The Battle.net account id, as a string of digits.
    pub id: String,
    /// `Name#1234`.
    pub battletag: String,
}

/// Longest BattleTag kept: 12 characters, `#` and up to 7 digits, with room to spare.
const MAX_BATTLETAG: usize = 64;

/// Reads `/userinfo`: `{"sub": "123", "id": 123, "battletag": "Name#1234"}`. The id comes from
/// `id` (a number) or `sub` (a string). `None` without an id or a BattleTag.
#[must_use]
pub fn parse_userinfo(v: &Value) -> Option<Identity> {
    let id = match &v["id"] {
        Value::Number(n) => n.as_u64().map(|n| n.to_string()),
        _ => None,
    }
    .or_else(|| v["sub"].as_str().map(str::to_owned))
    .filter(|id| !id.is_empty() && id.bytes().all(|c| c.is_ascii_digit()))?;
    let battletag = v["battletag"]
        .as_str()
        .map(str::trim)
        .filter(|t| {
            !t.is_empty() && t.chars().count() <= MAX_BATTLETAG && !t.contains(char::is_control)
        })?
        .to_owned();
    Some(Identity { id, battletag })
}

/// The login cookie's value: the `state` sent to Battle.net and the page to return to,
/// hex-encoded so any path fits in a cookie.
#[must_use]
pub fn login_cookie(state: &str, next: &str) -> String {
    let hex = next.bytes().fold(String::new(), |mut hex, b| {
        let _ = write!(hex, "{b:02x}");
        hex
    });
    format!("{state}.{hex}")
}

/// Reads a login cookie back: the state, and a page on this site to return to (`/` when the
/// page is missing, mangled or elsewhere).
#[must_use]
pub fn read_login_cookie(cookie: &str) -> Option<(&str, String)> {
    let (state, hex) = cookie.split_once('.')?;
    if state.is_empty() {
        return None;
    }
    let bytes: Option<Vec<u8>> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok())
        .collect();
    let next = bytes.and_then(|b| String::from_utf8(b).ok());
    Some((state, safe_next(next.as_deref()).to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use serde_json::json;

    #[test]
    fn reads_userinfo() {
        let me = Identity {
            id: "424242".into(),
            battletag: "Testhero#1234".into(),
        };
        let both = json!({"sub": "424242", "id": 424_242, "battletag": "Testhero#1234"});
        assert_eq!(parse_userinfo(&both), Some(me.clone()));
        let sub_only = json!({"sub": "424242", "battletag": " Testhero#1234 "});
        assert_eq!(parse_userinfo(&sub_only), Some(me));
        for bad in [
            json!({"battletag": "Testhero#1234"}),
            json!({"id": 1}),
            json!({"id": 1, "battletag": ""}),
            json!({"sub": "12a", "battletag": "x#1"}),
            json!({"id": -1, "battletag": "x#1"}),
            json!({"id": 1, "battletag": "x\n#1"}),
            json!("nope"),
        ] {
            assert_eq!(parse_userinfo(&bad), None, "{bad}");
        }
    }

    #[test]
    fn login_cookies() {
        let cookie = login_cookie("abc123", "/s/xyz?tag=raid&sort=level");
        assert!(
            cookie
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'.')
        );
        assert_eq!(
            read_login_cookie(&cookie),
            Some(("abc123", "/s/xyz?tag=raid&sort=level".into()))
        );
        // Somewhere else, mangled or missing: back to the front page.
        let elsewhere = login_cookie("abc", "//evil.example");
        assert_eq!(read_login_cookie(&elsewhere), Some(("abc", "/".into())));
        assert_eq!(read_login_cookie("abc.zz"), Some(("abc", "/".into())));
        assert_eq!(read_login_cookie("abc.2"), Some(("abc", "/".into())));
        assert_eq!(read_login_cookie("abc."), Some(("abc", "/".into())));
        assert_eq!(read_login_cookie("abc"), None);
        assert_eq!(read_login_cookie(".2f"), None);
    }

    proptest! {
        #[test]
        fn login_cookies_round_trip(state in "[0-9a-f]{1,64}", next in ".{0,200}") {
            let cookie = login_cookie(&state, &next);
            let (s, back) = read_login_cookie(&cookie).unwrap();
            prop_assert_eq!(s, state.as_str());
            prop_assert_eq!(back, safe_next(Some(&next)));
        }

        #[test]
        fn any_cookie_returns_to_this_site(cookie in ".{0,200}") {
            if let Some((_, next)) = read_login_cookie(&cookie) {
                prop_assert_eq!(safe_next(Some(&next)), next.as_str());
            }
        }
    }
}
