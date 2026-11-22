-- People who log in with Battle.net, and their sessions. Only the Battle.net account id and
-- BattleTag are kept; a session is kept as its token's SHA-256, so the database alone can't
-- log anyone in. Times are Unix seconds.

CREATE TABLE users (
    id         INTEGER PRIMARY KEY,
    bnet_id    TEXT NOT NULL UNIQUE,  -- Battle.net account id
    battletag  TEXT NOT NULL,         -- as Battle.net last said it
    created_at INTEGER NOT NULL,
    seen_at    INTEGER NOT NULL       -- last login
) STRICT;

CREATE TABLE sessions (
    token_hash TEXT PRIMARY KEY,      -- hex SHA-256 of the cookie's token
    user_id    INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL
) STRICT;

CREATE INDEX sessions_user ON sessions (user_id);
