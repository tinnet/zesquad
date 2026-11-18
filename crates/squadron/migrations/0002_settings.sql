-- Server-side values that must survive restarts, e.g. the key session cookies are derived from.
CREATE TABLE settings (
    name  TEXT PRIMARY KEY,
    value TEXT NOT NULL
) STRICT;
