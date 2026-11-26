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
| [Forum: "WoW: Forever API: what works so far"][forum-works] | Community notes on rulesets and auction houses, 2026-11-05/06 | Unofficial |
| [Goldbook issue #96][goldbook-houses] | Auction house ids probed in US; labels PvP/PvE houses, 2026-11-09 | Unofficial; the labels are the author's guess |

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
- **Authorization code** (a player's token): for logging in, and for `/profile/user/wow...`.
  Redirect to `https://oauth.battle.net/authorize` with `response_type=code`, `client_id`,
  `redirect_uri`, `state` and `scope` (space-separated). `redirect_uri` must be one registered
  with the client, or `https://localhost` for testing. Back at the redirect with `code` and
  `state`, swap the code at `POST https://oauth.battle.net/token` (basic auth as above,
  `grant_type=authorization_code`, `code`, the same `redirect_uri`); the token lasts 24 h
  (`expires_in: 86399`) and the answer lists the scopes granted. "The user may not grant access
  to any or all requested scopes", so the list may be shorter. [oauth], [oauth-code], [oauth-apis]
- **Scopes:** "Applications do not require any additional scopes to use Battle.net login.
  Without scopes, applications can only gain access to a user's account ID and BattleTag."
  `openid` is "access details about a currently logged in user using OpenID Connect", at
  `GET https://oauth.battle.net/userinfo`; `wow.profile` is "access to a user's World of
  Warcraft characters" (`/profile/user/wow`, protected characters, collections). Every endpoint
  a player's token opens is a GET: nothing can be changed on their behalf. [oauth], [oauth-code]
- **`/userinfo`** "returns a user's Account ID and BattleTag": `{"sub": "<id>", "id": <id>,
  "battletag": "Name#1234"}`. zesquad logs in with `scope=openid` and keeps those two only.
  [oauth-code]
- Checked against the docs on 2026-11-22. They render with JavaScript, so a plain fetch gets an
  empty page; a headless browser shows the text.
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
| Guild roster | `/data/wow/guild/{guild-slug}/roster` | **200**: `_links`, `guild`, `members[] {rank, character {key, name, surname, id, slug, level, playable_class, playable_race, faction}}`. The whole roster in one response, no paging (see Verified) |
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
- **Item levels can be missing.** On 2026-11-25, 8 of 18 summaries zesquad fetched had no
  `average_item_level` or `equipped_item_level` key, and their `equipment` items had no
  `level`. They were exactly the characters last logged in at or after 2026-11-25 05:12 UTC;
  every one last logged in at or before 2026-11-24 22:35 UTC still had both. The cut falls
  around the weekly Wednesday maintenance, so profiles saved since then seem to leave item
  levels out, at levels 14 to 60 alike. Not yet known whether Blizzard will bring them back.
- The summary links to `achievements`, `titles`, `pvp_summary`, `encounters`, `media`,
  `specializations`, `statistics`, `mythic_keystone_profile`, `equipment`, `appearance`,
  `collections`, `reputations`, `quests`, `achievements_statistics`, `professions`.
- `ruleset` (dynamic namespace) is new with Forever; "Normal" is the only value seen so far;
  the cards show its `name`. The ruleset index lists the others (see Auction houses).

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

## Auction houses

Forever has no realms (see Verified), so auctions hang off **rulesets** instead, in
`dynamic-forever-{region}`. Auctions went live around 2026-11-06. [forum-works]

| Path | Answer |
|------|--------|
| `/data/wow/ruleset/index` | 1 PvP, 2 Normal, 3 Roleplay, 15 Roleplay PvP |
| `/data/wow/ruleset/{id}` | `id`, `name`, `status {type: "UP"}`, a link to `.../auctions/` |
| `/data/wow/ruleset/{id}/auctions/index` | 15 houses: `id` and `name` (Alliance / Horde / Blackwater Auction House) |
| `/data/wow/ruleset/{id}/auctions/{house}` | Item auctions: `id`, `item {id, context?, modifiers?}`, `bid?`, `buyout?`, `quantity`, `time_left` |
| `/data/wow/ruleset/{id}/auctions/{house}/commodities` | Stackables: `id`, `item {id}`, `quantity`, `unit_price`, `time_left` |

- **The ruleset in the path doesn't filter anything.** Every ruleset lists the same 15 house ids,
  and house 110 under rulesets 1 and 2 returned the same 148,941 auctions. [forum-works], verified
- **House ids are the same in EU and US**: Alliance 110, 111, 112, 119, 123; Horde 113, 114,
  115, 120, 124; Blackwater (neutral) 116, 117, 118, 121, 125. [goldbook-houses], verified
- **Which house serves which servers isn't in the API.** By size there are two big houses per
  faction, a PvP and a PvE one (Goldbook labels 110/113 PvP and 111/114 PvE), then smaller ones,
  probably the roleplay rulesets. Item counts on 2026-11-20 (ruleset 1):

  | Houses | EU | US |
  |--------|----|----|
  | 110, 111 (Alliance) | 148,941; 153,067 | 91,641; 96,731 |
  | 113, 114 (Horde) | 152,064; 143,936 | 99,147; 105,875 |
  | 119, 123 / 120, 124 | 12-23k each | 17-25k each |
  | 116, 117 (Blackwater) | 14,845; 16,314 | 9,244; 10,104 |
  | 121, 125 (Blackwater) | 382; 378 | 431; 609 |
  | 112, 115, 118 | 0 | 0 |
- **Items and commodities are separate**: no auction id or item id is in both dumps of a house.
  Item auctions all had `quantity` 1; 5 of 148,941 had no `buyout` (bid only).
- **Auction ids and `time_left`** are what zesquad estimates sales from (`zesquad_core::sales`):
  an auction gone by the next dump, with more time left than passed, was bought or cancelled.
  `time_left` is `SHORT`, `MEDIUM`, `LONG` or `VERY_LONG`, the bands the game shows: under 30
  minutes, 30 minutes to 2 hours, 2 to 12 hours, over 12 hours (retail's `AUCTION_TIME_LEFT*`
  strings). **Not verified on Forever** yet: the bands, that an auction keeps its id while it's
  up, and that a partly bought commodity auction keeps its id with a smaller `quantity` (as on
  retail). The archived dumps can check all three.
- **Size**: one big house is about 150k item auctions (28 MB of formatted JSON) plus 125k
  commodity auctions (19 MB); about 2.6 MB gzipped. Both dumps arrive in a few seconds with
  `Accept-Encoding: gzip`.
- **How often it changes**: about hourly, all followed houses in both regions together, at no
  fixed minute: new dumps showed up between about :20 and :35 past the hour (our server's logs,
  November 2026). Both dumps of a house carry the same `Last-Modified`.
- **Conditional requests work**: responses have `Last-Modified` and
  `Cache-Control: public, max-age=86400`, no `ETag`. Sending that `Last-Modified` back as
  `If-Modified-Since` gets `304` with an empty body; an older date gets the full `200`. Tried by
  hand on EU and US house 110, items and commodities, 2026-11-20;
  `reads_real_auction_houses_in_eu_and_us` in `tests/network.rs` checks it.
- The forum says the index needs its trailing slash (`.../auctions/`). Our client drops trailing
  slashes, and `.../auctions/index` works.
- `/data/wow/auctions/commodities` (retail's region-wide commodities) answers 404.
- The forum reports that 43% of the item ids on an EU Alliance house 404 on the item endpoint.
  Not verified.

## Items

Item details are in the static namespaces: `GET /data/wow/item/{id}`. Forever has its own,
`static-forever-{region}` (build `static-1.60.1_70399-forever-eu` on 2026-11-25), but it lacks
many Classic items, so zesquad falls back to Classic Era's `static-classic1x-{region}`.
Tried with our client on the EU host, 2026-11-25:

- An item answers `id`, `name`, `quality {type, name}`, `item_class {id, name}`,
  `item_subclass {id, name}`, `inventory_type {type, name}`, `level`, `required_level`,
  `sell_price` and `purchase_price` (copper), `is_stackable`, `is_equippable`, a `media` link
  and a `preview_item` tooltip. `crates/zesquad-core/fixtures/item.json` is Light Leather (2318).
- **About 7,600 items** were on sale in EU and US houses 110 and 113 together: 6,174 in the item
  dumps and 1,426 commodities. 1,234 of them have ids over 100,000, items new with Forever.
- **About 41% aren't in Forever's item data**: 62 of 150 random ids answered 404 (the forum
  says 43%). [forum-works] Most are Classic world-drop gear, such as random-enchant greens.
  22 of 25 of those were in `static-classic1x-eu` and `static-classic-eu`; the other 3 (ids
  over 280,000) were in no namespace, retail's `static-eu` included.
- **The top-level `sell_price` can be wrong in Forever's item data; `preview_item.sell_price.value`
  (the tooltip's) is what the game shows.** Of 4,606 Forever items stored on 2026-11-25, 105
  had a top-level 0 with a tooltip price (Red Dragonscale Leggings, 252603: 0 against 42,088)
  and 15 had both set but different (Hammer of the Northern Wind, 810: 45,267 against 47,649).
  425 had no tooltip price: 248 of those had a top-level 0 (quest items and the like, which
  can't be sold) and 176 had 1 (ammunition). In Classic Era's item data the two always agreed.
  `crates/zesquad-core/fixtures/item-epic.json` is item 252603.
- `/data/wow/media/item/{id}` gives the icon: `assets [{key: "icon", value, file_data_id}]`,
  with URLs like `https://render.worldofwarcraft.com/forever-eu/icons/56/{file_data_id}.jpg`.
  Forever's media has icons for items its item data lacks (9950 and 264077 answered, though
  9950's item 404s). Classic Era's media names icons by file name instead
  (`.../classic1x-eu/icons/56/inv_misc_leatherscrap_03.jpg`).
- The render host serves icons at 18, 36 and 56 pixels (0.9-1.7 KB JPEGs; `large` is 403),
  with `Access-Control-Allow-Origin: *`; an unknown file id is 403. 134400 is the game's red
  question mark (`INV_Misc_QuestionMark`). Tried 2026-11-25.
- `/data/wow/search/item` answers 404 in `static-forever-eu` and `static-classic-eu`, so items
  are looked up one by one.
- A Forever namespace of another region answers 403 on the EU host (`static-forever-us`).

## Limits and errors

- 36,000 requests per hour per client, 100 per second. Going over the per-second limit gives 429
  for the rest of that second; going over the hourly quota slows responses. [getting-started]
  Whether a 429 carries `Retry-After` isn't documented or seen yet; zesquad honours it if so.
- 404 means unknown character **or** a character that hasn't logged out since its data was
  published; the API doesn't tell them apart.

## Verified with our client

Requests made with zesquad's own client credentials (app token, EU host) on 2026-11-17, two
weeks after launch; the auction rows on 2026-11-20, on the EU and US hosts. `just api <path>`
repeats any of them.

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

| `/data/wow/auctions`, `/auctions/index`, `/region/93/auctions` (`dynamic-forever-eu`, 2026-11-20) | 404, empty body: not routed |
| `/data/wow/auctions/commodities`, `/connected-realm/93/auctions[/index]` (same) | 404, JSON body |
| `/data/wow/ruleset/index`, `/ruleset/{1,2,3,15}/auctions/index` (EU and US, 2026-11-20) | **200**, see Auction houses |
| `/data/wow/ruleset/{1,2}/auctions/{house}[/commodities]` for every house id, EU and US | **200**, see Auction houses |
| `/data/wow/guild/{slug}`, `/roster` for a real guild | **200**, see Endpoints |
| `/data/wow/guild/{slug}`, `/roster` for a big US guild (`profile-forever-us`, 2026-11-23) | **200**: `member_count` 823 and 823 `members`, no paging keys; about 650 KB indented. Ranks 0-6, 0 is the guild master |
| `/profile/wow/character/{name}-{surname}[/...]` for real level 10-30 characters | **200** for the summary and every sub-endpoint listed above, except `hunter-pets` (404; neither is a hunter) |

An unrouted path gives an empty 404, which is what the forum saw for `/{name}-{surname}` before
launch. Now the path is routed: real characters return data, and unknown ones get the JSON
"not found" 404 that Classic Era gives too.

## What zesquad does with this

- Squad members are entered as `Name-Surname`, in their squad's region (EU, US or TW).
- A background job fetches the summary, character-media, equipment, specializations,
  professions and, for hunters, hunter-pets in `profile-forever-{region}` with the app token (5-6
  requests per character, well within the limits), keeps each raw response, and shows level,
  race, class, faction, guild, item level, last login, avatar, talents, professions, gear and
  pets. A 404 on the summary shows "not on
  the Armory": a typo, or a character that hasn't logged out since it was created.
- With `ZESQUAD_AUCTIONS`, a second background job follows auction houses 110, 111, 113 and 114
  in EU and US (both factions, PvP and PvE). It reads both dumps of a house about hourly and
  keeps prices per item (lowest, median by units, quantity, auctions) for a week, and condensed
  per day for good. Not yet: the roleplay and Blackwater houses, item names.
- A guild's page (`/g/{region}/{slug}`, linked from character cards) fetches the guild and its
  roster: 2 requests per guild, not one per member. It shows the roster's own name, level,
  class, race and rank, refreshed while people keep opening the page. Possible later: import a
  guild's roster into a squad.
- Not built: "sign in with Battle.net" to import a player's characters through
  `/profile/user/wow`. Not needed now that the public lookup works.

[getting-started]: https://community.developer.battle.net/documentation/guides/getting-started
[oauth]: https://community.developer.battle.net/documentation/guides/using-oauth
[oauth-cc]: https://community.developer.battle.net/documentation/guides/using-oauth/client-credentials-flow
[oauth-code]: https://community.developer.battle.net/documentation/guides/using-oauth/authorization-code-flow
[oauth-apis]: https://community.developer.battle.net/documentation/battle-net/oauth-apis
[regionality]: https://community.developer.battle.net/documentation/guides/regionality-and-apis
[namespaces]: https://community.developer.battle.net/documentation/world-of-warcraft/guides/namespaces
[classic-profile]: https://community.developer.battle.net/documentation/world-of-warcraft-classic/profile-apis
[classic-data]: https://community.developer.battle.net/documentation/world-of-warcraft-classic/game-data-apis
[renders]: https://community.developer.battle.net/documentation/world-of-warcraft/guides/character-renders
[forum-forever]: https://us.forums.blizzard.com/en/blizzard/t/world-of-warcraft-forever-api-namespace/59774
[tierlist]: https://wowforevertierlist.com/armory
[forum-works]: https://us.forums.blizzard.com/en/blizzard/t/wow-forever-api-what-works-so-far-plus-a-ts-client/60334
[goldbook-houses]: https://github.com/jkc-be/goldbook/issues/96
