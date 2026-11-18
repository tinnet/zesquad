# Development

## Setup

```sh
just setup                 # mise install (Rust, just, lefthook, betterleaks, git-cliff) + git hooks
cp .env.example .env       # add Battle.net client credentials (optional for most work)
just run                   # http://127.0.0.1:8080, database in data/
```

`just` lists every task.

## Tests

- `just test`: unit and property tests in `squadron-core`, and black-box HTTP tests in
  `crates/squadron/tests/http.rs`. Those start the real app on a random port with a temporary
  database and a fake Battle.net in the same process, serving the made-up character in
  `crates/squadron-core/fixtures/`. No network needed.
- `just network-test`: calls the real API with `.env`'s credentials. Set
  `SQUADRON_TEST_CHARACTER=Name-Surname` (in `.env` is fine) to a Forever character in EU.
- `just ci`: format check, clippy (pedantic, warnings are errors), tests. CI runs exactly this.

Fixtures and tests use made-up characters only; real names never go in the repo.

## Exploring the Battle.net API

`just api <path> [namespace]` prints any API response with the app token, e.g.

```sh
just api /data/wow/playable-race/index static-forever-eu
just api /profile/wow/character/<name>-<surname>/statistics
```

Record what you learn in [battlenet-api.md](battlenet-api.md), with the source.

## Hooks

lefthook (installed by `just setup`):

- pre-commit: betterleaks on staged changes, `just fmt` for Rust files and the justfile.
- commit-msg: Conventional Commits.
- pre-push: every pushed commit is signed.

## CI

`.github/workflows/ci.yml` on pushes to `main` and on PRs: `just ci`; the static musl binary for
x86_64 and aarch64, started once as a smoke test; and `gh actions-lock --verify`. Every action is
pinned in `.github/workflows/actions.lock`: after changing a `uses:`, run `gh actions-lock` and
commit the lockfile with it. Dependabot proposes action and crate updates weekly.

## Releases

CalVer tags, `vYYYY.MM.DD` (`.N` for a second release that day). `just release` (from a clean
`main` that matches `origin/main`) writes `CHANGELOG.md` with git-cliff, commits, and pushes a
signed tag. `.github/workflows/release.yml` then:

1. builds `x86_64-unknown-linux-musl` and `aarch64-unknown-linux-musl` on native runners and
   checks `squadron --version` prints the tag's version (shadow-rs reads it from git);
2. publishes `squadron-linux-<arch>.tar.gz`, `SHA256SUMS` and build provenance as a GitHub
   release with the git-cliff notes;
3. pushes `ghcr.io/tinnet/squadron:<version>` and `:latest` for amd64 and arm64. The
   `Dockerfile` only copies the prebuilt binaries onto distroless.

`Cargo.toml`'s `version` stays `0.1.0`; untagged builds report `dev`.

`just docker` builds the image locally (needs `cargo-zigbuild` and zig for the musl build on
macOS).
