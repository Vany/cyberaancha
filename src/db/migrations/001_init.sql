-- P1: credentials and the meta kv-store. Domain tables arrive with their phases.

-- Secrets live here, not in config (SPEC §13): argon2 PHC strings for panel
-- roles, blake3 hex for bearer tokens. `name` doubles as basic-auth username.
CREATE TABLE auth (
    name       TEXT PRIMARY KEY,
    kind       TEXT NOT NULL CHECK (kind IN ('password', 'token')),
    hash       TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- Watermarks, clocks (last_gathered_at / last_processed_at / last_backup_*), misc state.
CREATE TABLE meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
