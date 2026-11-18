//! Where the login page may send people afterwards.

/// `next` if it is a path on this site, `/` otherwise, so the login page can't be used to send
/// people elsewhere.
///
/// Browsers drop tabs and newlines from a URL and treat `\` like `/`, so `/\t/evil.example` and
/// `/\evil.example` both mean `//evil.example`: another site. Control characters and
/// backslashes never appear in our own paths (they arrive percent-encoded), so any is refused.
#[must_use]
pub fn safe_next(next: Option<&str>) -> &str {
    match next {
        Some(n)
            if n.starts_with('/')
                && !n.starts_with("//")
                && !n.contains(|c: char| c == '\\' || c.is_control()) =>
        {
            n
        }
        _ => "/",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// What a browser makes of a `Location` before resolving it (WHATWG URL): leading and
    /// trailing C0 controls and spaces go, tabs and newlines go everywhere, `\` is `/`.
    fn as_browser_reads(location: &str) -> String {
        location
            .trim_matches(|c: char| c <= ' ')
            .chars()
            .filter(|c| !matches!(c, '\t' | '\n' | '\r'))
            .map(|c| if c == '\\' { '/' } else { c })
            .collect()
    }

    #[test]
    fn next_stays_on_site() {
        assert_eq!(safe_next(Some("/s/abc?tag=raid")), "/s/abc?tag=raid");
        assert_eq!(
            safe_next(Some("/login?next=%2Fs%2Fx")),
            "/login?next=%2Fs%2Fx"
        );
        assert_eq!(safe_next(Some("//evil.example")), "/");
        assert_eq!(safe_next(Some("https://evil.example")), "/");
        assert_eq!(safe_next(Some("/\\evil.example")), "/");
        assert_eq!(safe_next(Some("/\t/evil.example")), "/");
        assert_eq!(safe_next(Some("/\n/evil.example")), "/");
        assert_eq!(safe_next(Some("/\r\n/evil.example")), "/");
        assert_eq!(safe_next(Some("")), "/");
        assert_eq!(safe_next(None), "/");
    }

    proptest! {
        #[test]
        fn browsers_read_every_result_as_a_path_here(s in "[/\\\\\t\n\r a-z.:%?=]{0,12}") {
            let next = safe_next(Some(&s));
            let read = as_browser_reads(next);
            prop_assert!(read.starts_with('/') && !read.starts_with("//"), "{s:?} -> {next:?}");
        }

        #[test]
        fn ordinary_paths_pass_unchanged(s in "/[a-z0-9]([a-z0-9/?=&%-]{0,30})") {
            prop_assert_eq!(safe_next(Some(&s)), s.as_str());
        }
    }
}
