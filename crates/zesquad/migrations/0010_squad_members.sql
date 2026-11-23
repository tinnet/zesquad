-- Squads get members, one of them the owner, and a visibility (see zesquad_core::access).
-- Squads from before are `open`: anyone with the link sees and edits them, as before, until
-- someone logged in opens one and becomes its owner. The invite token is the secret in
-- `/join/<token>`; it's shown to members, so it's kept as it is. Times are Unix seconds.

ALTER TABLE squads ADD COLUMN visibility TEXT NOT NULL DEFAULT 'open'
    CHECK (visibility IN ('open', 'private', 'unlisted', 'public'));
ALTER TABLE squads ADD COLUMN invite_token TEXT;

CREATE UNIQUE INDEX squads_invite_token ON squads (invite_token);

CREATE TABLE squad_members (
    squad_id  TEXT NOT NULL REFERENCES squads (id) ON DELETE CASCADE,
    user_id   INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    role      TEXT NOT NULL CHECK (role IN ('owner', 'member')),
    joined_at INTEGER NOT NULL,
    PRIMARY KEY (squad_id, user_id)
) STRICT;

CREATE INDEX squad_members_user ON squad_members (user_id);
-- One owner per squad.
CREATE UNIQUE INDEX squad_members_owner ON squad_members (squad_id) WHERE role = 'owner';
