-- Sales estimated by matching auction ids from one dump of a house to the next (see
-- zesquad_core::sales). Units and copper; times in seconds.

-- The auctions in each house's last stored snapshot, to compare the next dump with:
-- zesquad_core::sales::encode, zstd-compressed, a few MB for a big house. `taken` is when
-- Battle.net made the dumps (their Last-Modified).
CREATE TABLE auction_listings (
    region TEXT NOT NULL,
    house INTEGER NOT NULL,
    taken INTEGER NOT NULL,
    listings BLOB NOT NULL,
    PRIMARY KEY (region, house)
);

-- How long before a snapshot the one it was compared with was taken; NULL when it wasn't
-- compared (the first, or after a gap).
ALTER TABLE auction_snapshot_times ADD COLUMN compared INTEGER;

-- A row per item sold since the snapshot before, kept for a week like auction_snapshots.
CREATE TABLE auction_sales (
    region TEXT NOT NULL,
    house INTEGER NOT NULL,
    at INTEGER NOT NULL,
    item INTEGER NOT NULL,
    sold INTEGER NOT NULL,
    value INTEGER NOT NULL,
    PRIMARY KEY (region, house, at, item)
) WITHOUT ROWID;

-- A day's sales per item, condensed from auction_sales and kept for good.
CREATE TABLE auction_daily_sales (
    region TEXT NOT NULL,
    house INTEGER NOT NULL,
    day INTEGER NOT NULL,
    item INTEGER NOT NULL,
    sold INTEGER NOT NULL,
    value INTEGER NOT NULL,
    PRIMARY KEY (region, house, day, item)
) WITHOUT ROWID;

-- How much of each condensed day was compared, so its sales can be read as a rate.
CREATE TABLE auction_days (
    region TEXT NOT NULL,
    house INTEGER NOT NULL,
    day INTEGER NOT NULL,
    compared INTEGER NOT NULL,
    PRIMARY KEY (region, house, day)
) WITHOUT ROWID;
