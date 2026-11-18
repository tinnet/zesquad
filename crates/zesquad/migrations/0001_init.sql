-- Squads, their characters and tags, and the Armory data fetched for those characters.
-- Times are Unix seconds.

CREATE TABLE squads (
    id         TEXT PRIMARY KEY,   -- random UUID v4; knowing it is the only access control
    name       TEXT NOT NULL,
    created_at INTEGER NOT NULL
) STRICT;

CREATE TABLE characters (
    id         INTEGER PRIMARY KEY,
    squad_id   TEXT NOT NULL REFERENCES squads (id) ON DELETE CASCADE,
    region     TEXT NOT NULL,
    name       TEXT NOT NULL,
    surname    TEXT NOT NULL,
    char_key   TEXT NOT NULL,      -- lower-case name-surname, see CharacterName::key
    added_at   INTEGER NOT NULL,
    UNIQUE (squad_id, region, char_key)
) STRICT;

CREATE TABLE character_tags (
    character_id INTEGER NOT NULL REFERENCES characters (id) ON DELETE CASCADE,
    tag_key      TEXT NOT NULL,    -- lower-case, for comparing
    label        TEXT NOT NULL,    -- as first entered
    PRIMARY KEY (character_id, tag_key)
) STRICT;

-- One row per character across all squads, so a character in five squads is fetched once.
CREATE TABLE armory (
    region     TEXT NOT NULL,
    char_key   TEXT NOT NULL,
    status     TEXT NOT NULL DEFAULT 'pending',
    fetched_at INTEGER,
    error      TEXT,
    PRIMARY KEY (region, char_key)
) STRICT;

-- The raw JSON of each endpoint from the last successful fetch, so pages can show more later
-- without fetching again.
CREATE TABLE armory_documents (
    region   TEXT NOT NULL,
    char_key TEXT NOT NULL,
    endpoint TEXT NOT NULL,
    body     TEXT NOT NULL,
    PRIMARY KEY (region, char_key, endpoint),
    FOREIGN KEY (region, char_key) REFERENCES armory (region, char_key) ON DELETE CASCADE
) STRICT;
