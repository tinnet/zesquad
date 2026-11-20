-- One row per stored snapshot of a house, so pages can count snapshots and look a few items up
-- in each snapshot by key, instead of scanning a week of rows.

CREATE TABLE auction_snapshot_times (
    region TEXT NOT NULL,
    house INTEGER NOT NULL,
    at INTEGER NOT NULL,
    -- Items in the snapshot.
    items INTEGER NOT NULL,
    PRIMARY KEY (region, house, at)
) WITHOUT ROWID;

INSERT INTO auction_snapshot_times (region, house, at, items)
SELECT region, house, at, COUNT(*) FROM auction_snapshots GROUP BY region, house, at;
