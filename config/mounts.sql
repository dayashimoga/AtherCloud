-- AetherStream SQLite Virtual Mount Table Schema (WAL Mode)
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS virtual_mounts (
    mount_id TEXT PRIMARY KEY NOT NULL,
    physical_root TEXT NOT NULL,
    permission TEXT NOT NULL CHECK(permission IN ('READ', 'WRITE', 'STREAM_ONLY')),
    description TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now', 'utc')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now', 'utc'))
);

CREATE INDEX IF NOT EXISTS idx_mount_permission ON virtual_mounts(permission);