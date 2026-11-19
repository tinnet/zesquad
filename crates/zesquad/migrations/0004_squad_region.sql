-- Each squad has a region, and its characters are looked up there. Squads from before take
-- their characters' region; empty ones get the configured default when the database is opened
-- (`db::open`), which SQL can't know.

ALTER TABLE squads ADD COLUMN region TEXT;

UPDATE squads SET region = (SELECT c.region FROM characters c WHERE c.squad_id = squads.id LIMIT 1);
