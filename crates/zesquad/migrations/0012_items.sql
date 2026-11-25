-- Item details from Battle.net's static game data (see zesquad_core::items), for every item seen
-- at an auction house. The same in every region.

CREATE TABLE items (
    id INTEGER PRIMARY KEY,
    -- Last time Battle.net was asked about it; NULL until it has been.
    fetched_at INTEGER,
    -- Which item data had it: 'forever' or 'classic'. NULL when none had it, or not asked yet.
    source TEXT,
    -- The parts pages search and sort by; NULL like source.
    name TEXT,
    quality TEXT,
    class_id INTEGER,
    subclass_id INTEGER,
    -- The whole document, so showing another field needs no refetch.
    json TEXT,
    -- Why the last ask failed; NULL when it worked (found or not).
    error TEXT
);

-- Finding the items to ask about next.
CREATE INDEX items_fetched_at ON items (fetched_at);

INSERT OR IGNORE INTO items (id) SELECT DISTINCT item FROM auction_daily;
INSERT OR IGNORE INTO items (id) SELECT DISTINCT item FROM auction_snapshots;
