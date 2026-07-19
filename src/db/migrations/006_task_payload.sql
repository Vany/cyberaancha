-- Per-task payload (JSON). First use: transcribe tasks carry {"whisper":true}
-- when integrate rejected the auto-captions, telling the Mac to skip yt-dlp subs
-- and transcribe the audio directly.
ALTER TABLE tasks ADD COLUMN payload_json TEXT;
