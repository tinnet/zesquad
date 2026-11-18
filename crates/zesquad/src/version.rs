//! The version, from the release tag (CalVer) via shadow-rs; `dev` for untagged builds.

use std::sync::LazyLock;

shadow_rs::shadow!(build);

/// `2026.11.17` for a release built from tag `v2026.11.17`, `dev` otherwise.
#[must_use]
pub fn short() -> &'static str {
    let tag = build::TAG.trim_start_matches('v');
    if tag.is_empty() { "dev" } else { tag }
}

/// Distinguishes static file URLs between builds: the version for releases, the commit for
/// dev builds.
#[must_use]
pub fn asset_tag() -> &'static str {
    if build::TAG.is_empty() {
        if build::SHORT_COMMIT.is_empty() {
            "dev"
        } else {
            build::SHORT_COMMIT
        }
    } else {
        short()
    }
}

/// For `--version`: the version plus commit and build details.
///
/// shadow-rs's `CLAP_LONG_VERSION` starts with the Cargo package version, which we never bump;
/// its first line is swapped for [`short`] so `--version` and `-V` agree.
#[must_use]
pub fn long() -> &'static str {
    static LONG: LazyLock<String> = LazyLock::new(|| {
        let details = build::CLAP_LONG_VERSION
            .split_once('\n')
            .map_or("", |(_, rest)| rest);
        format!("{}\n{details}", short())
    });
    &LONG
}
