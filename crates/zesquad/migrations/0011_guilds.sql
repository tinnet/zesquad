-- Guilds someone opened a page for, and their guild and roster documents, like `armory` and
-- `armory_documents` for characters. Kept apart from squads.

CREATE TABLE guilds (
    region     TEXT NOT NULL,
    slug       TEXT NOT NULL,
    status     TEXT NOT NULL DEFAULT 'pending',
    fetched_at INTEGER,
    error      TEXT,
    -- When the page was last opened: guilds nobody looks at stop being refreshed.
    viewed_at  INTEGER NOT NULL,
    PRIMARY KEY (region, slug)
) STRICT;

CREATE TABLE guild_documents (
    region   TEXT NOT NULL,
    slug     TEXT NOT NULL,
    endpoint TEXT NOT NULL,   -- guild, roster
    body     TEXT NOT NULL,
    PRIMARY KEY (region, slug, endpoint),
    FOREIGN KEY (region, slug) REFERENCES guilds (region, slug) ON DELETE CASCADE
) STRICT;
