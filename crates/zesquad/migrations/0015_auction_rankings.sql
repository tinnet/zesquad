-- Each region's auction overview, ranked after its data changes, so the page reads a few
-- kilobytes instead of ranking ~90k rows on every visit. Derived: rows can be deleted at any
-- time and are ranked again (see crate::ranking).

CREATE TABLE auction_rankings (
    region TEXT PRIMARY KEY,
    -- The format and rules it was ranked with (crate::ranking::VERSION).
    version INTEGER NOT NULL,
    -- What it was ranked from: each house's latest snapshot and last condensed day, and when
    -- item details last changed (JSON, compared as is).
    source TEXT NOT NULL,
    -- The ranking, JSON.
    ranked TEXT NOT NULL,
    ranked_at INTEGER NOT NULL
);
