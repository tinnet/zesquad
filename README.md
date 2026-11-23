# zesquad

Friend squads for **WoW: Forever**. A squad is a page with a name and the characters you play
with, each tagged however you like (`raid`, `alts`, `tank`, ...). zesquad looks every character
up on the Battle.net Armory and shows their level, race, class, guild, item level, talents,
professions, gear, hunter pets and when they last played.

- A squad lives at its own unguessable link (`/s/<random id>`). With logging in on (see below),
  whoever starts a squad owns it: anyone with the link can look, and friends who join with its
  invite link edit it. With logging in off there are no accounts, and anyone with the link can
  see and edit the squad, so share it with your friends only. (Squads started while logging in
  was on keep who can see them, and stay read-only until it's back.) Set `ZESQUAD_PASSWORD` to put one
  shared password in front of the whole site as well.
- Characters are entered as `Name-Surname`, the way Forever names them. EU only for now.
- Details refresh in the background every hour, or on demand with **Refresh**. Battle.net only
  updates a character when it logs out.

## Run it

zesquad is one static binary with everything built in, and a SQLite database file.

With Docker:

```sh
docker run -d -p 8080:8080 -v zesquad-data:/data \
  -e ZESQUAD_BNET_CLIENT_ID=... -e ZESQUAD_BNET_CLIENT_SECRET=... \
  ghcr.io/tinnet/zesquad:latest
```

Or download `zesquad-linux-<arch>.tar.gz` from the
[releases](https://github.com/tinnet/zesquad/releases) and run `zesquad serve`.

Armory lookups need a Battle.net API client: create one at
<https://community.developer.battle.net/access/clients> (it's free; your account needs an
authenticator). Without credentials zesquad still works, showing just names and tags.

To let people log in with Battle.net, set `ZESQUAD_PUBLIC_URL` and add
`<public URL>/auth/callback` as a redirect URL of the same API client. Logging in asks Battle.net
for nothing but the account id and BattleTag.

## Configuration

Environment variables (or the matching `--flags` of `zesquad serve`; a `.env` file in the
working directory is read too):

| Variable                      | Default             | Meaning                                         |
|-------------------------------|---------------------|-------------------------------------------------|
| `ZESQUAD_BNET_CLIENT_ID`     |                     | Battle.net API client id                        |
| `ZESQUAD_BNET_CLIENT_SECRET` |                     | Battle.net API client secret                    |
| `ZESQUAD_BNET_REGION`        | `eu`                | region new squads start in: `eu`, `us`, `tw`    |
| `ZESQUAD_PASSWORD`           |                     | shared password for the whole site; empty: none |
| `ZESQUAD_PUBLIC_URL`         |                     | where people reach zesquad, e.g. `https://zesquad.example`; turns on logging in with Battle.net, and links squads in Discord posts |
| `ZESQUAD_TRUST_PROXY`       | `none`              | behind a reverse proxy, where it puts each client's address, for the rate limits: `cloudflare` (`CF-Connecting-IP`) or `forwarded-for` (last `X-Forwarded-For` entry: Caddy, nginx, HAProxy). Without a proxy, or with the wrong one, clients could pick their address. Behind a proxy without it, all clients share one set of limits |
| `ZESQUAD_LISTEN`             | `127.0.0.1:8080`    | address to listen on (`0.0.0.0:8080` in Docker) |
| `ZESQUAD_DATABASE`           | `data/zesquad.db`  | SQLite file, created if missing                 |
| `ZESQUAD_REFRESH_MINUTES`    | `60`                | how often a character's details are refreshed   |
| `ZESQUAD_AUCTIONS`           | `false`             | follow auction house prices (EU, US)            |
| `ZESQUAD_ARCHIVE_DIR`        |                     | keep every auction dump (zstd) in this folder   |
| `ZESQUAD_ARCHIVE_S3_URL`     |                     | upload them to this bucket, path-style (`https://ENDPOINT/BUCKET`) |
| `ZESQUAD_ARCHIVE_S3_REGION`  | `auto`              | the bucket's signing region (`auto` for R2)     |
| `ZESQUAD_ARCHIVE_S3_ACCESS_KEY_ID`     |           | the bucket's access key id                      |
| `ZESQUAD_ARCHIVE_S3_SECRET_ACCESS_KEY` |           | the bucket's secret access key                  |
| `ZESQUAD_LOG_JSON`           | `false`             | log JSON lines instead of text                  |
| `RUST_LOG`                    | `info`              | log level, e.g. `zesquad=debug`                |

Back up the database file (or the `/data` volume). `/healthz` answers 200 while the database is
reachable; the image's health check uses `zesquad healthcheck`.

## Development

See [docs/development.md](docs/development.md). What the Battle.net API offers for Forever, and
how we know, is in [docs/battlenet-api.md](docs/battlenet-api.md).

## License

MIT
