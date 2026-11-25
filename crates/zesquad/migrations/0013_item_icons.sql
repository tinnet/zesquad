-- Item icons, from Battle.net's item media (see zesquad_core::items::icon). Looked up apart from
-- the details, since Forever's media has icons for items its item data lacks.

-- The icon's file id; NULL when there's none, or not looked up yet.
ALTER TABLE items ADD COLUMN icon INTEGER;
-- Last time the media was asked for; NULL until it has been.
ALTER TABLE items ADD COLUMN icon_fetched_at INTEGER;
-- Why the last ask failed; NULL when it worked (found or not).
ALTER TABLE items ADD COLUMN icon_error TEXT;

-- Finding the icons to ask about next.
CREATE INDEX items_icon_fetched_at ON items (icon_fetched_at);
