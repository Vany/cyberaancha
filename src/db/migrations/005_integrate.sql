-- P4b: mark videos whose knowledge has been integrated into the KB, and add a
-- transcribe task path (preparer). integrate is a preparer task keyed by video;
-- it collapses SPEC's extract+integrate into one Claude pass (recorded in MEMO)
-- — the session reads the video bundle, searches the KB, and writes articles.

ALTER TABLE videos ADD COLUMN integrated INTEGER NOT NULL DEFAULT 0;
-- set when captions are unusable and a Whisper transcript is pending/done
ALTER TABLE videos ADD COLUMN transcribe_state TEXT NOT NULL DEFAULT 'no'
    CHECK (transcribe_state IN ('no', 'pending', 'done'));
