# Design

## Crates

- `zesquad-core`: no I/O. Character names (`Name-Surname`, normalised; the lower-case key is
  what the database and the API use), tags (case-insensitive, first spelling kept), squad
  names, when a character is due for a refresh, reading Battle.net JSON into a `Profile`, and
  the tags and tag colours that come from it.
  Parsing is lenient because Forever's responses are undocumented.
- `zesquad`: the binary. `cli` (clap, every option also an env var), `db` (sqlx + SQLite, all
  queries), `armory` (Battle.net client), `sync` (background refresh), `web` (axum handlers,
  askama templates), `assets` (CSS and htmx embedded with rust-embed).

## Data

```mermaid
erDiagram
    squads ||--o{ characters : has
    squads ||--o{ tag_colours : picks
    characters ||--o{ character_tags : has
    characters }o--|| armory : "region + char_key"
    armory ||--o{ armory_documents : stores
```

- A character belongs to one squad; the same character in two squads is two rows.
- `armory` holds one row per `(region, char_key)`, shared by every squad with that character, so
  each character is fetched once. Rows for characters no squad has any more are kept but not
  refreshed.
- `tag_colours` holds the colours picked for a squad's tags, by tag key, separate from the
  tags themselves: a colour stays when a tag goes away and comes back.
- `armory_documents` keeps each endpoint's raw JSON from the last good fetch; the pages parse it
  the first time a character is shown after a fetch and keep the result in memory. Showing a new
  field needs no refetch, and a failed fetch keeps the old data.

## Armory sync

One task loops: fetch every due character, then sleep until woken (a character added or a
refresh asked for) or a minute passes. Due means never fetched or asked for (`pending`),
or older than the interval (found / not found), or a tenth of it after a failure.

Per character it fetches the summary first; a 404 there marks the character not found. Then
media, equipment, specializations, professions and, for hunters, hunter pets. A 404 on one of
those is skipped; any other error marks the whole fetch failed. The app token is cached until
five minutes before it expires (halfway, for one that lives shorter), and a 401 gets one retry
with a new token; a second 401 fails the fetch.

## Tags

A character's tags are the ones people typed (`character_tags`) plus automatic ones from its
Armory data: class, main spec, race, faction and every profession (`zesquad_core::tags`). The
automatic ones are worked out when the page renders, never stored, so they follow the character
and can't be removed; a character the Armory hasn't answered for has none. The main spec is the
talent tree with the most points in the active group, if no other tree ties it. A typed tag
the Armory also gives is shown once, as automatic, and comes back as a typed one if the
character loses the automatic tag. Since tags aren't all in the database, the roster filters
by tag after loading the squad.

A tag's colour is the one picked for it in the squad, else its default: class tags default to
the classic client's class colours (`RAID_CLASS_COLORS`), others to none. Chip text is black or
white, whichever contrasts more with the colour (WCAG relative luminance).

## Web

Every action is a plain HTML form POST that redirects back (so it works without JavaScript).
With htmx, the same forms post with `HX-Request` and get the re-rendered roster section back,
which replaces the old one; validation errors come back the same way, with what was typed kept.
While any character is still pending, the roster polls itself every 3 seconds.

Static files are served under `/static/<version>/` with a one-year cache in release builds, so a
new version is never stuck behind old CSS.

A squad's id, a random UUID v4 written in Crockford base32 (`squad_id.rs` in core), is its
secret; squads made before that have the dashed UUID form, which still works. Anything that
changes a squad checks the character belongs to that squad. Optionally, `ZESQUAD_PASSWORD`
puts one shared password in front of everything (`auth.rs`, an axum middleware). Logging in sets an HttpOnly, `SameSite=Lax` cookie holding
`sha256(key, 0, password)`, where the key is random and kept in the `settings` table: sessions
survive restarts, the cookie isn't the password, and changing the password logs everyone out.
`/login`, `/logout`, `/healthz`, `/version` and static files stay open. htmx requests without a
session get `HX-Redirect` to the login page. A wrong password costs 750 ms, to slow guessing; put
a reverse proxy with rate limiting in front for more. The cookie is `Secure` when the proxy says
the request came over HTTPS (`X-Forwarded-Proto`).
