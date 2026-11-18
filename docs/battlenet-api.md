# Battle.net API for WoW: Forever

What zesquad can learn about a character from Blizzard's API, where each fact comes from, and
how sure we are. Forever launched on 2026-11-04 and Blizzard hasn't documented its API: what's
here is the documented Classic API, community probing, and our own requests (see "Verified").
Update this file when something changes.

## Sources

| Source | What it is | Trust |
|--------|------------|-------|
| [Getting started][getting-started] | Clients, URIs, throttling | Official |
| [Using OAuth][oauth], [client credentials flow][oauth-cc], [authorization code flow][oauth-code] | Tokens and scopes | Official |
| [Regionality and APIs][regionality] | Hosts, regions, locales | Official |
| [WoW namespaces][namespaces] | `static` / `dynamic` / `profile` categories | Official (retail names only) |
| [WoW Classic Profile APIs][classic-profile] | Character and account endpoints for Classic | Official (lists `profile-classic-*` only) |
| [WoW Classic Game Data APIs][classic-data] | Realms, classes, races, items | Official |
| [Character renders][renders] | Avatar images and fallbacks | Official |
| [Forum: "World of Warcraft: Forever API Namespace?"][forum-forever] | Community probe of Forever namespaces, 2026-11-02 (pre-launch) | Unofficial, unconfirmed by Blizzard |
| [wowforevertierlist.com/armory][tierlist] | Third-party status page, as of 2026-10-18 | Unofficial |

The developer portal renders its docs with JavaScript, so a plain fetch of the pages above returns
an empty shell. The content is served as JSON at
`https://community.developer.battle.net/api/pages/content/documentation/<page>.json` (for example
`.../world-of-warcraft-classic/profile-apis.json`): `.html` holds the prose, and `.resources` holds
the endpoint list with every path and parameter.

## Authentication

- **Client credentials** (the app's own token): `POST https://oauth.battle.net/token` with HTTP
  basic auth `client_id:client_secret` and form body `grant_type=client_credentials`. The response
  is `{"access_token", "token_type": "bearer", "expires_in": 86399, ...}`, so a token lasts about
  24 h; cache it and fetch a new one shortly before it expires. [oauth-cc]
- Send it as `Authorization: Bearer <token>`. [oauth-cc]
- **Authorization code** (a player's token): needed only for `/profile/user/wow...` endpoints.
  Redirect to `https://oauth.battle.net/authorize` with `response_type=code`, `client_id`,
  `redirect_uri`, `state`, and `scope=wow.profile`, which grants "access to a user's World of Warcraft
  characters". Players can untick scopes, so the scopes granted may be fewer than requested.
  [oauth], [oauth-code]
- The client needs a Battle.net account with an authenticator, and the API terms must be accepted.
  [getting-started]

## Hosts, regions, locales

- EU host: `https://eu.api.blizzard.com/`; locales `en_GB`, `de_DE`, `fr_FR`, `es_ES`, `it_IT`,
  `pt_PT`, `ru_RU`. Without `locale`, localised strings come back as an object with every
  locale; with `?locale=en_GB` they're plain strings. [regionality]
- Data is per region: an EU character only exists in EU. [regionality]

## Namespaces

Every WoW request needs a namespace, as the header `Battlenet-Namespace: <ns>` or `?namespace=<ns>`.
[namespaces]

| Game | Profile | Dynamic | Static | Source |
|------|---------|---------|--------|--------|
| Retail | `profile-eu` | `dynamic-eu` | `static-eu` | [namespaces] |
| Classic (progression) | `profile-classic-eu` | `dynamic-classic-eu` | `static-classic-eu` | [classic-profile], [classic-data] |
| Classic Era | `profile-classic1x-eu` | `dynamic-classic1x-eu` | `static-classic1x-eu` | Not in the official docs; [tierlist], community usage |
| **Forever** | `profile-forever-eu` | `dynamic-forever-eu` | `static-forever-eu` (resolves to e.g. `static-1.60.1_70399-forever-eu`) | [forum-forever]; verified below |

Forever specifics, from the forum probe (all unconfirmed):

- Before launch, plain `static-forever-eu` returned 403 and needed the build in it. Since launch it
  works and resolves to the versioned one (see "Verified").
- Works in US, EU, TW; KR returns 403.
- Character data updates on logout and doesn't exist for a character that has never logged in.
  (That's documented for all profile namespaces too.) [namespaces]

## Characters on Forever: no realm, a surname

Forever characters have `name` and `surname` and **no realm** in the account profile. Their links
look like:

```json
"character": { "href": "https://eu.api.blizzard.com/profile/wow/character/{name}-{surname}?namespace=profile-forever-eu" },
"protected_character": { "href": "https://eu.api.blizzard.com/profile/user/wow/protected-character/{realmId}-{characterId}?namespace=profile-forever-eu" },
"name": "...", "surname": "..."
```

So the hidden realm survives as the `realmId` in the protected-character link. [forum-forever]

The public lookup takes the place of `{realmSlug}/{characterName}` with one segment, the
lower-case `{name}-{surname}`: `/profile/wow/character/{name}-{surname}[/...]`. Verified with
real characters (see "Verified").

## Endpoints

All paths on `https://eu.api.blizzard.com`, `namespace=profile-forever-eu`, app token unless
marked. The paths come from the [Classic profile API][classic-profile] (and retail, for the ones
Classic doesn't list); the status column is what we saw on Forever on 2026-11-17.

| Endpoint | Path | Forever status |
|----------|------|----------------|
| Account profile summary | `/profile/user/wow` (player token) | Works [forum-forever]; 403 with an app token |
| Account collections | `/profile/user/wow/collections` (player token) | Works [forum-forever] |
| Character profile summary | `/profile/wow/character/{name}-{surname}` | **200** |
| Status | `.../status` | **200**: `{"id", "is_valid": true}` |
| Appearance | `.../appearance` | **200**: customizations, displayed items |
| Equipment | `.../equipment` | **200** |
| Hunter pets | `.../hunter-pets` | **200** for hunters: `hunter_pets[] {name, level, creature {name}, slot, is_active, is_summoned}`; 404 for other classes |
| Media (renders) | `.../character-media` | **200** |
| PvP summary | `.../pvp-summary` | **200**: `honorable_kills`, `pvp_rank` |
| Specializations (talents) | `.../specializations` | **200** |
| Statistics | `.../statistics` | **200**: health, power, primary stats, armor, crit, dodge, weapon damage... |
| Achievements | `.../achievements`, `.../achievements/statistics` | **200**: Forever has its own achievements (ids 62000+) |
| Professions | `.../professions` | **200** (retail endpoint, not in the Classic docs) |
| Reputations | `.../reputations` | **200** (retail endpoint) |
| Quests | `.../quests` | **200**: `in_progress` list (retail endpoint) |
| Titles, encounters, collections, mythic keystone | `.../titles`, `.../encounters`, ... | 200 but empty or index-only so far |
| Guild | `/data/wow/guild/{guild-slug}` | **200**, realmless like characters: `id`, `name`, `name_search`, `faction`, `member_count`, `achievement_points`, `created_timestamp`, `ruleset` |
| Guild roster | `/data/wow/guild/{guild-slug}/roster` | **200**: `members[] {rank, character {name, surname, id, slug, level, playable_class, playable_race, faction}}` |
| Guild achievements, activity | `/data/wow/guild/{guild-slug}/achievements`, `.../activity` | 200, empty so far |

Static and dynamic data (classes, races, items, realms) are in the [Classic Game Data APIs][classic-data];
on Forever there are no realms.

### Character profile summary

`GET /profile/wow/character/{name}-{surname}?namespace=profile-forever-eu&locale=en_GB`, shape
with made-up values and the `_links` / `href` fields left out:

```json
{
  "id": 1234567,
  "name": "Aldric",
  "surname": "Thornwood",
  "name_search": "Aldric Thornwood",
  "gender": { "type": "FEMALE", "name": "Female" },
  "faction": { "type": "ALLIANCE", "name": "Alliance" },
  "race": { "name": "Night Elf", "id": 4 },
  "character_class": { "name": "Rogue", "id": 4 },
  "level": 26,
  "experience": 4991,
  "achievement_points": 0,
  "last_login_timestamp": 1794955893000,
  "average_item_level": 12,
  "equipped_item_level": 12,
  "guild": { "name": "Some Guild", "id": 98765, "slug": "some-guild",
             "faction": { "type": "ALLIANCE", "name": "Alliance" } },
  "ruleset": { "name": "Normal", "id": 2 },
  "is_remix": false
}
```

- No `realm`; `guild` is missing when the character has none. `last_login_timestamp` is in ms.
- The summary links to `achievements`, `titles`, `pvp_summary`, `encounters`, `media`,
  `specializations`, `statistics`, `mythic_keystone_profile`, `equipment`, `appearance`,
  `collections`, `reputations`, `quests`, `achievements_statistics`, `professions`.
- `ruleset` (dynamic namespace) is new with Forever; "Normal" is the only value seen so far.

### Other responses zesquad reads

- **character-media**: `assets: [{key, value}]` with keys `avatar`, `avatar-sd`, `main-raw`,
  `main-sd-raw`; URLs like `https://render.worldofwarcraft.com/forever-eu/character/31/<id>-avatar.jpg`.
  Every endpoint's `character` object repeats `name`, `id`, `surname` and `slug`.
- **equipment**: `equipped_items[]` with `slot {type, name}`, `name`, `quality {type, name}`
  (`POOR`, `COMMON`, `UNCOMMON`, `RARE`, `EPIC`, ...), `level {value}`, `item {id}`, and tooltip
  details (armor, stats, durability, requirements, sell price).
- **specializations**: `specialization_groups[] {is_active, specializations[] {specialization_name,
  spent_points, talents[] {talent {name}, talent_rank, spell_tooltip}}}`, the Classic talent trees.
- **professions**: `primaries[]` / `secondaries[]` with `profession {name}` and
  `tiers[] {skill_points, max_skill_points}`.

Character renders: `character-media` lists `avatar`, `inset`, `main` image URLs, generated on
logout. A missing avatar can fall back to a generic one with
`?alt=/shadow/avatar/{race-id}-{gender-id}.jpg` (gender 0 male, 1 female). [renders]

## Limits and errors

- 36,000 requests per hour per client, 100 per second. Going over the per-second limit gives 429
  for the rest of that second; going over the hourly quota slows responses. [getting-started]
- 404 means unknown character **or** a character that hasn't logged out since its data was
  published; the API doesn't tell them apart.

## Verified with our client

Requests made with zesquad's own client credentials (app token, EU host) on 2026-11-17, two
weeks after launch. `just api <path>` repeats any of them.

| Request | Result |
|---------|--------|
| `POST https://oauth.battle.net/token` (client credentials) | 200, token |
| `/data/wow/region/index?namespace=dynamic-forever-eu` | 200: one region, id 93 |
| `/data/wow/region/93?namespace=dynamic-forever-eu` | 200: `"name": "CAMWOW Europe", "tag": "EU", "patch_string": "1.60.1"` |
| `/data/wow/realm/index`, `/data/wow/connected-realm/index` (`dynamic-forever-eu`) | 404: no realms published |
| `/data/wow/search/realm?namespace=dynamic-forever-eu` | 200, zero results |
| `/data/wow/playable-class/index?namespace=static-forever-eu` | 200. **Plain `static-forever-eu` works now** (the forum saw 403 before launch); links point at `static-1.60.1_70399-forever-eu` |
| ... classes | 1 Warrior, 2 Paladin, 3 Hunter, 4 Rogue, 5 Priest, 7 Shaman, 8 Mage, 9 Warlock, 11 Druid |
| `/data/wow/playable-race/index?namespace=static-forever-eu` | 1 Human, 2 Orc, 3 Dwarf, 4 Night Elf, 5 Undead, 6 Tauren, 7 Gnome, 8 Troll, plus two new ones: 95 High Order Skyborne, 96 Windshaper Skyborne |
| `/profile/wow/character/madeup-nobodyhere?namespace=profile-forever-eu` | 404 **with a JSON body** `{"code":404,"type":"BLZWEBAPI00000404","detail":"Not Found"}` and a `battlenet-namespace` header |
| `/this/path/does/not/exist?namespace=profile-forever-eu` | 404, **empty body** |
| `/profile/wow/character/amnennar/madeupnobody?namespace=profile-classic1x-eu` | same JSON 404 as the Forever lookup |
| `/profile/wow/character/x-y?namespace=profile-forever-us` on the EU host | 403: namespace region must match the host |
| `/profile/user/wow?namespace=profile-forever-eu` | 403: needs a player token, as documented |

| `/data/wow/guild/{slug}`, `/roster` for a real guild | **200**, see Endpoints |
| `/profile/wow/character/{name}-{surname}[/...]` for real level 10-30 characters | **200** for the summary and every sub-endpoint listed above, except `hunter-pets` (404; neither is a hunter) |

An unrouted path gives an empty 404, which is what the forum saw for `/{name}-{surname}` before
launch. Now the path is routed: real characters return data, and unknown ones get the JSON
"not found" 404 that Classic Era gives too.

## What zesquad does with this

- Squad members are entered as `Name-Surname`, region EU.
- A background job fetches the summary, character-media, equipment, specializations,
  professions and, for hunters, hunter-pets in `profile-forever-eu` with the app token (5-6
  requests per character, well within the limits), keeps each raw response, and shows level,
  race, class, faction, guild, item level, last login, avatar, talents, professions, gear and
  pets. A 404 on the summary shows "not on
  the Armory": a typo, or a character that hasn't logged out since it was created.
- Possible later: import a guild's roster into a squad (the roster lists every member's
  name and surname).
- Not built: "sign in with Battle.net" to import a player's characters through
  `/profile/user/wow`. Not needed now that the public lookup works.

[getting-started]: https://community.developer.battle.net/documentation/guides/getting-started
[oauth]: https://community.developer.battle.net/documentation/guides/using-oauth
[oauth-cc]: https://community.developer.battle.net/documentation/guides/using-oauth/client-credentials-flow
[oauth-code]: https://community.developer.battle.net/documentation/guides/using-oauth/authorization-code-flow
[regionality]: https://community.developer.battle.net/documentation/guides/regionality-and-apis
[namespaces]: https://community.developer.battle.net/documentation/world-of-warcraft/guides/namespaces
[classic-profile]: https://community.developer.battle.net/documentation/world-of-warcraft-classic/profile-apis
[classic-data]: https://community.developer.battle.net/documentation/world-of-warcraft-classic/game-data-apis
[renders]: https://community.developer.battle.net/documentation/world-of-warcraft/guides/character-renders
[forum-forever]: https://us.forums.blizzard.com/en/blizzard/t/world-of-warcraft-forever-api-namespace/59774
[tierlist]: https://wowforevertierlist.com/armory
