# zesquad

Friend squads for **WoW: Forever**. A squad is a page with a name and the characters you play
with, each tagged however you like (`raid`, `alts`, `tank`, ...). zesquad looks every character
up on the Battle.net Armory and shows their level, race, class, guild, item level, talents,
professions, gear, hunter pets and when they last played.

- No accounts: a squad lives at its own unguessable link (`/s/<uuid>`). Anyone with the link can
  see and edit it, so share it with your friends only. Set `ZESQUAD_PASSWORD` to put one shared
  password in front of the whole site as well.
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

## Configuration

Environment variables (or the matching `--flags` of `zesquad serve`; a `.env` file in the
working directory is read too):

| Variable                      | Default             | Meaning                                         |
|-------------------------------|---------------------|-------------------------------------------------|
| `ZESQUAD_BNET_CLIENT_ID`     |                     | Battle.net API client id                        |
| `ZESQUAD_BNET_CLIENT_SECRET` |                     | Battle.net API client secret                    |
| `ZESQUAD_BNET_REGION`        | `eu`                | `eu`, `us` or `tw`                              |
| `ZESQUAD_PASSWORD`           |                     | shared password for the whole site; empty: none |
| `ZESQUAD_LISTEN`             | `127.0.0.1:8080`    | address to listen on (`0.0.0.0:8080` in Docker) |
| `ZESQUAD_DATABASE`           | `data/zesquad.db`  | SQLite file, created if missing                 |
| `ZESQUAD_REFRESH_MINUTES`    | `60`                | how often a character's details are refreshed   |
| `ZESQUAD_LOG_JSON`           | `false`             | log JSON lines instead of text                  |
| `RUST_LOG`                    | `info`              | log level, e.g. `zesquad=debug`                |

Back up the database file (or the `/data` volume). `/healthz` answers 200 while the database is
reachable; the image's health check uses `zesquad healthcheck`.

## Development

See [docs/development.md](docs/development.md). What the Battle.net API offers for Forever, and
how we know, is in [docs/battlenet-api.md](docs/battlenet-api.md).

## License

MIT
