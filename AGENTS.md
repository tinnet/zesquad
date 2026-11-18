# AGENTS.md

Guidance for coding agents (and humans) working in this repository. The [README](README.md) is
for users. [`docs/design.md`](docs/design.md) explains the internals,
[`docs/development.md`](docs/development.md) the build, tests and releases, and
[`docs/battlenet-api.md`](docs/battlenet-api.md) what the Battle.net API offers for WoW: Forever
and how we know.

## Setup and commands

Tools are pinned in `mise.toml`; tasks live in the `justfile`.

```sh
just setup         # mise install + lefthook install
just test          # unit, property and HTTP tests; Battle.net is faked
just ci            # what CI runs: format check, clippy, tests
just fmt           # rustfmt + `just --fmt`
just run           # serve on http://127.0.0.1:8080, database in data/
just api <path>    # GET a Battle.net path with the app token (needs .env)
```

Run `just ci` before declaring work done. Battle.net credentials go in `.env` (copy
`.env.example`); it's gitignored. `just network-test` calls the real API.

## Code

- `crates/squadron-core`: pure logic (names, tags, refresh timing, parsing Battle.net JSON). No
  I/O. `crates/squadron`: the server, database, Battle.net client and templates.
- Clippy pedantic, warnings fail the build. Fix the cause; don't `#[allow]` without a comment
  saying why. `unsafe` is forbidden.
- New behaviour gets a test that drives it over real HTTP (`crates/squadron/tests/`), against
  the fake Battle.net server there. Parsing and validation get unit and property tests in core.
- Battle.net facts go in `docs/battlenet-api.md` with their source.
- Schema changes are new files in `crates/squadron/migrations/`; never edit an applied one.

## Privacy

This repo is public. Never commit real character names, guild names, character ids or
Battle.net credentials, including in test fixtures, docs, logs or commit messages. Fixtures use
made-up names.

## Commits

[Conventional Commits](https://www.conventionalcommits.org): `<type>[(scope)][!]: <description>`,
enforced by the commit-msg hook. Types: `feat`, `fix`, `perf`, `refactor`, `test`, `docs`,
`build`, `ci`, `chore`, `style`, `revert`. Scopes are optional (`web`, `armory`, `db`, `core`).
Description lower case, imperative, no trailing period; the body says why, wrapped at 72.
`feat`, `fix`, `perf` and `docs` subjects become the changelog, so write them for users.

Every commit is signed; the pre-push hook rejects unsigned ones. One logical change per commit,
tests in the same commit. The pre-commit hook scans staged changes for secrets.

## Releases

CalVer, `vYYYY.MM.DD` (`.N` for another release that day). `just release` writes
`CHANGELOG.md`, commits, tags and pushes from a clean `main`; the release workflow builds the
Linux binaries and the container image. Don't tag any other way, and never move or delete a
published tag.
