//! Full links to pages, for copying and sharing.

/// `scheme://host` from a request's `Host` (or `X-Forwarded-Host`) and whether it came over
/// HTTPS; `None` when the host doesn't look like a host name, an IPv4 or a bracketed IPv6
/// address, with an optional port. The host comes from the request, so it's only ever shown
/// back to whoever sent it.
#[must_use]
pub fn origin(https: bool, host: &str) -> Option<String> {
    let ok = !host.is_empty()
        && host.len() <= 260
        && !host.starts_with([':', '.', '-'])
        && host
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b".-:[]".contains(&c));
    ok.then(|| format!("{}://{host}", if https { "https" } else { "http" }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn origins() {
        assert_eq!(
            origin(true, "squads.example").as_deref(),
            Some("https://squads.example")
        );
        assert_eq!(
            origin(false, "127.0.0.1:8080").as_deref(),
            Some("http://127.0.0.1:8080")
        );
        assert_eq!(
            origin(false, "[::1]:8080").as_deref(),
            Some("http://[::1]:8080")
        );
        for bad in [
            "",
            "evil.example/path",
            "a b",
            "x\"y",
            "user@host",
            ":80",
            "a\\b",
        ] {
            assert_eq!(origin(true, bad), None, "{bad}");
        }
    }

    proptest! {
        #[test]
        fn an_origin_is_only_scheme_and_host(https in any::<bool>(), host in ".{0,40}") {
            if let Some(o) = origin(https, &host) {
                let rest = o.split_once("://").unwrap().1;
                prop_assert_eq!(rest, host.as_str());
                prop_assert!(!rest.contains(['/', '@', '?', '#', '\\', '"', '<']));
            }
        }
    }
}
