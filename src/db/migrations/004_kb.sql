-- P4: the knowledge base. Articles are the wiki; each has a one-paragraph
-- answer (paragraph_ru) and a full story (story_md). Stances form the dated
-- opinion timeline; aliases drive recall in pure BM25; facts carry provenance
-- and authority; links cross-reference; questions are the professor's queue.

CREATE TABLE articles (
    slug         TEXT PRIMARY KEY,              -- human-readable id, set by integrate
    title        TEXT NOT NULL,
    paragraph_ru TEXT NOT NULL DEFAULT '',      -- the bot answer, professor's voice, <=~800 chars
    story_md     TEXT NOT NULL DEFAULT '',      -- full narrative for her to read/verify
    status       TEXT NOT NULL DEFAULT 'draft' CHECK (status IN ('draft', 'published')),
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL
);

-- Build-time recall boosters: synonyms, misspellings, latin/EN drug & gene
-- names, typical phrasings. This is what makes «боль в заднице» find «геморрой».
CREATE TABLE article_aliases (
    article_slug TEXT NOT NULL REFERENCES articles (slug) ON DELETE CASCADE,
    alias        TEXT NOT NULL,
    PRIMARY KEY (article_slug, alias)
);

-- Dated opinion timeline. "Переосмыслено в <link>" is reconstructed from these.
CREATE TABLE stances (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    article_slug TEXT NOT NULL REFERENCES articles (slug) ON DELETE CASCADE,
    text         TEXT NOT NULL,
    video_id     TEXT,                          -- source video for the citation link
    offset_ms    INTEGER,                       -- timemark within the video
    source_kind  TEXT NOT NULL CHECK (source_kind IN ('video', 'comment', 'chat', 'panel')),
    source_ref   TEXT,                          -- comment/chat id when applicable
    authority    TEXT NOT NULL CHECK (authority IN ('panel', 'comment_author', 'spoken', 'inferred')),
    occurred_at  TEXT,                          -- when she said it (for recency weighting)
    created_at   TEXT NOT NULL
);
CREATE INDEX stances_by_article ON stances (article_slug);

CREATE TABLE facts (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    article_slug TEXT NOT NULL REFERENCES articles (slug) ON DELETE CASCADE,
    text         TEXT NOT NULL,
    source_kind  TEXT NOT NULL CHECK (source_kind IN ('video', 'comment', 'chat', 'panel')),
    source_ref   TEXT,
    authority    TEXT NOT NULL CHECK (authority IN ('panel', 'comment_author', 'spoken', 'inferred')),
    confidence   REAL,
    created_at   TEXT NOT NULL
);
CREATE INDEX facts_by_article ON facts (article_slug);

CREATE TABLE article_links (
    from_slug TEXT NOT NULL REFERENCES articles (slug) ON DELETE CASCADE,
    to_slug   TEXT NOT NULL REFERENCES articles (slug) ON DELETE CASCADE,
    kind      TEXT NOT NULL CHECK (kind IN ('related', 'parent', 'contradicts')),
    PRIMARY KEY (from_slug, to_slug, kind)
);

-- The professor's queue: contradictions and gaps found at integrate time.
CREATE TABLE questions (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    article_slug TEXT REFERENCES articles (slug) ON DELETE SET NULL,
    context      TEXT NOT NULL,
    question     TEXT NOT NULL,
    answer       TEXT,
    status       TEXT NOT NULL DEFAULT 'open' CHECK (status IN ('open', 'answered', 'dismissed')),
    created_at   TEXT NOT NULL,
    answered_at  TEXT
);
CREATE INDEX questions_by_status ON questions (status);

-- Every search (test tab now, bot later): misses feed questions + alias fixes.
CREATE TABLE queries (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    q          TEXT NOT NULL,
    hit_slug   TEXT,                            -- best article, or NULL on a miss
    score      REAL,
    created_at TEXT NOT NULL
);
