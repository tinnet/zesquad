-- Battle.net's Last-Modified of each dump in the last stored snapshot of a house, sent back as
-- If-Modified-Since so a dump that hasn't changed isn't downloaded again.

ALTER TABLE auction_houses ADD COLUMN items_modified TEXT;
ALTER TABLE auction_houses ADD COLUMN commodities_modified TEXT;
