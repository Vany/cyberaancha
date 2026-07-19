-- P3: comments and live-chat replay. Both carry an is_author flag
-- (authorChannelId == the channel) — the professor's own replies are top
-- authority for the KB (SPEC §6). Stored structured (not raw blobs): the
-- structured rows ARE the data and comments/chat are bulky on a tight disk.

ALTER TABLE videos ADD COLUMN comments_state TEXT NOT NULL DEFAULT 'pending'
    CHECK (comments_state IN ('pending', 'have', 'none'));
-- 'na' for non-streams (no live chat); streams start 'pending'.
ALTER TABLE videos ADD COLUMN chat_state TEXT NOT NULL DEFAULT 'na'
    CHECK (chat_state IN ('pending', 'have', 'none', 'na'));

CREATE TABLE comments (
    yt_id             TEXT PRIMARY KEY,          -- YouTube comment id
    video_id          TEXT NOT NULL REFERENCES videos (yt_id),
    parent_id         TEXT,                      -- NULL for top-level, else parent comment id
    author_channel_id TEXT,
    author_name       TEXT NOT NULL DEFAULT '',
    text              TEXT NOT NULL DEFAULT '',
    like_count        INTEGER,
    published_at      TEXT,                      -- exact if given, else approx from relative text
    is_author         INTEGER NOT NULL DEFAULT 0,
    fetched_at        TEXT NOT NULL
);
CREATE INDEX comments_by_video ON comments (video_id);
CREATE INDEX comments_by_author ON comments (author_channel_id);
CREATE INDEX comments_author_replies ON comments (is_author) WHERE is_author = 1;

CREATE TABLE chat_messages (
    yt_id             TEXT PRIMARY KEY,          -- chat message/action id
    video_id          TEXT NOT NULL REFERENCES videos (yt_id),
    offset_ms         INTEGER,                   -- ms into the stream (replay timing)
    author_channel_id TEXT,
    author_name       TEXT NOT NULL DEFAULT '',
    text              TEXT NOT NULL DEFAULT '',
    is_author         INTEGER NOT NULL DEFAULT 0,
    fetched_at        TEXT NOT NULL
);
CREATE INDEX chat_by_video ON chat_messages (video_id);
CREATE INDEX chat_by_author ON chat_messages (author_channel_id);
