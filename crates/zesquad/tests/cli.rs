//! The command line itself, run as the built binary.

use std::process::Command;

fn run(arg: &str) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_zesquad"))
        .arg(arg)
        .output()
        .expect("run zesquad");
    assert!(out.status.success(), "zesquad {arg}: {out:?}");
    String::from_utf8(out.stdout).expect("utf-8 version")
}

/// `--version` leads with the same version as `-V` (the release tag, or `dev`), not the Cargo
/// package version; the release smoke test relies on it.
#[test]
fn long_version_starts_with_the_short_version() {
    let short = run("-V");
    let long = run("--version");
    let short = short.trim_end();
    assert!(short.starts_with("zesquad "), "{short}");
    assert_ne!(short, "zesquad 0.1.0", "-V shows the Cargo version");
    assert_eq!(long.lines().next(), Some(short), "{long}");
    assert!(long.contains("commit_hash:"), "{long}");
}
