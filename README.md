# squadron

Friend squads for **WoW: Forever**. A squad is a page with a name and the characters you play
with, each tagged however you like (`raid`, `alts`, `tank`, ...). squadron looks every character
up on the Battle.net Armory and shows their level, race, class, guild, item level, talents,
professions, gear, hunter pets and when they last played.

- No accounts: a squad lives at its own unguessable link (`/s/<uuid>`). Anyone with the link can
  see and edit it, so share it with your friends only. Set `SQUADRON_PASSWORD` to put one shared
  password in front of the whole site as well.
- Characters are entered as `Name-Surname`, the way Forever names them. EU only for now.
- Details refresh in the background every hour, or on demand with **Refresh**. Battle.net only
  updates a character when it logs out.

## Run it

squadron is one static binary with everything built in, and a SQLite database file.

With Docker:

```sh
docker run -d -p 8080:8080 -v squadron-data:/data \
  -e SQUADRON_BNET_CLIENT_ID=... -e SQUADRON_BNET_CLIENT_SECRET=... \
  ghcr.io/tinnet/squadron:latest
```

Or download `squadron-linux-<arch>.tar.gz` from the
[releases](https://github.com/tinnet/squadron/releases) and run `squadron serve`.

Armory lookups need a Battle.net API client: create one at
<https://community.developer.battle.net/access/clients> (it's free; your account needs an
authenticator). Without credentials squadron still works, showing just names and tags.

## Configuration

Environment variables (or the matching `--flags` of `squadron serve`; a `.env` file in the
working directory is read too):

| Variable                      | Default             | Meaning                                         |
|-------------------------------|---------------------|-------------------------------------------------|
| `SQUADRON_BNET_CLIENT_ID`     |                     | Battle.net API client id                        |
| `SQUADRON_BNET_CLIENT_SECRET` |                     | Battle.net API client secret                    |
| `SQUADRON_BNET_REGION`        | `eu`                | `eu`, `us` or `tw`                              |
| `SQUADRON_PASSWORD`           |                     | shared password for the whole site; empty: none |
| `SQUADRON_LISTEN`             | `127.0.0.1:8080`    | address to listen on (`0.0.0.0:8080` in Docker) |
| `SQUADRON_DATABASE`           | `data/squadron.db`  | SQLite file, created if missing                 |
| `SQUADRON_REFRESH_MINUTES`    | `60`                | how often a character's details are refreshed   |
| `SQUADRON_LOG_JSON`           | `false`             | log JSON lines instead of text                  |
| `RUST_LOG`                    | `info`              | log level, e.g. `squadron=debug`                |

Back up the database file (or the `/data` volume). `/healthz` answers 200 while the database is
reachable; the image's health check uses `squadron healthcheck`.

## Development

See [docs/development.md](docs/development.md). What the Battle.net API offers for Forever, and
how we know, is in [docs/battlenet-api.md](docs/battlenet-api.md).

## License

MIT
