-- A squad can post its news (characters joining, level-ups) to a Discord channel. This is the
-- webhook's path on Discord, `/api/webhooks/<id>/<token>` (see zesquad_core::discord); the
-- token is a secret, so pages never show it back. NULL: no posting.

ALTER TABLE squads ADD COLUMN discord_webhook TEXT;
