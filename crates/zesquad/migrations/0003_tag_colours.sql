-- Colours picked for tags, per squad. A tag without a row here shows its default colour (class
-- tags) or none. Tags themselves live on characters, or come from the Armory, so this isn't
-- tied to them: a colour stays if the tag goes away and comes back.
CREATE TABLE tag_colours (
    squad_id TEXT NOT NULL REFERENCES squads (id) ON DELETE CASCADE,
    tag_key  TEXT NOT NULL,        -- lower-case, like character_tags.tag_key
    colour   TEXT NOT NULL,        -- #rrggbb
    PRIMARY KEY (squad_id, tag_key)
) STRICT;
