-- Auction house prices, per region and house (see zesquad_core::auctions). Prices are copper
-- per unit. Keys start with region, house and time, so each house's rows for a stretch of time
-- are next to each other: that's how they're written, condensed and pruned.

-- Where fetching each house stands.
CREATE TABLE auction_houses (
    region TEXT NOT NULL,
    house INTEGER NOT NULL,
    -- Last time the house was asked for, successfully or not.
    checked_at INTEGER,
    -- The last stored snapshot's fingerprint: a dump with the same one isn't stored again.
    fingerprint TEXT,
    -- The last day condensed into auction_daily (days since the epoch, UTC).
    condensed_through INTEGER,
    -- Why the last check failed; NULL when it worked.
    error TEXT,
    PRIMARY KEY (region, house)
) WITHOUT ROWID;

-- A row per item per snapshot (about hourly), kept for a week.
CREATE TABLE auction_snapshots (
    region TEXT NOT NULL,
    house INTEGER NOT NULL,
    at INTEGER NOT NULL,
    item INTEGER NOT NULL,
    min_price INTEGER NOT NULL,
    median_price INTEGER NOT NULL,
    quantity INTEGER NOT NULL,
    auctions INTEGER NOT NULL,
    PRIMARY KEY (region, house, at, item)
) WITHOUT ROWID;

-- A row per item per day, condensed from that day's snapshots and kept for good.
CREATE TABLE auction_daily (
    region TEXT NOT NULL,
    house INTEGER NOT NULL,
    day INTEGER NOT NULL,
    item INTEGER NOT NULL,
    min_price INTEGER NOT NULL,
    median_price INTEGER NOT NULL,
    max_price INTEGER NOT NULL,
    quantity INTEGER NOT NULL,
    snapshots INTEGER NOT NULL,
    PRIMARY KEY (region, house, day, item)
) WITHOUT ROWID;
