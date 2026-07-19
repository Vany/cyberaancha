-- P2: video inventory, typed task queue, raw harvest storage, transcripts.

CREATE TABLE videos (
    yt_id            TEXT PRIMARY KEY,          -- 11-char YouTube id
    channel_id       TEXT,
    kind             TEXT NOT NULL CHECK (kind IN ('video', 'stream')),
    title            TEXT NOT NULL DEFAULT '',
    approx_published TEXT,                      -- parsed from "N weeks ago"; batching heuristic only
    published_at     TEXT,                      -- exact, from harvest_meta
    duration_s       INTEGER,
    description      TEXT,
    view_count       INTEGER,
    meta_done        INTEGER NOT NULL DEFAULT 0,
    captions_state   TEXT NOT NULL DEFAULT 'pending'
                     CHECK (captions_state IN ('pending', 'have', 'none')),
    discovered_at    TEXT NOT NULL
);
CREATE INDEX videos_by_approx ON videos (approx_published);

-- Typed work queue (SPEC §5). UNIQUE(type, subject) makes enqueue idempotent.
CREATE TABLE tasks (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    type        TEXT NOT NULL,
    subject     TEXT NOT NULL,                  -- yt_id, or channel handle for discover
    state       TEXT NOT NULL DEFAULT 'pending'
                CHECK (state IN ('pending', 'claimed', 'done', 'failed')),
    attempt     INTEGER NOT NULL DEFAULT 0,
    lease_until TEXT,
    claimed_by  TEXT,
    error       TEXT,
    created_at  TEXT NOT NULL,
    done_at     TEXT,
    UNIQUE (type, subject)
);
CREATE INDEX tasks_claimable ON tasks (state, type, id);

-- Raw harvest payloads, zstd-compressed: debuggability + reprocessing without refetch.
CREATE TABLE raw_docs (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    video_id   TEXT NOT NULL REFERENCES videos (yt_id),
    kind       TEXT NOT NULL,                   -- 'captions_json3' | 'meta_player' | later: comments, chat
    content    BLOB NOT NULL,
    fetched_at TEXT NOT NULL
);
CREATE INDEX raw_docs_by_video ON raw_docs (video_id, kind);

-- One row per video: parsed timed segments as zstd JSON array.
CREATE TABLE transcripts (
    video_id      TEXT PRIMARY KEY REFERENCES videos (yt_id),
    source        TEXT NOT NULL CHECK (source IN ('asr', 'manual', 'whisper')),
    lang          TEXT NOT NULL,
    segments_zstd BLOB NOT NULL,
    updated_at    TEXT NOT NULL
);
