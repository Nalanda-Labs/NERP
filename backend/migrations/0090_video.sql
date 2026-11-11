-- Videos made playable in every browser: a web copy (H.264/AAC MP4, at most 1080p, starting
-- to play before it has all arrived) and a poster frame, made in the background.
CREATE TABLE file_renditions (
    file_id        UUID PRIMARY KEY REFERENCES files (id) ON DELETE CASCADE,
    -- pending | ready (web copy made) | original (already playable) | too_big | failed | unavailable
    status         TEXT NOT NULL,
    web_file_id    UUID REFERENCES files (id) ON DELETE SET NULL,
    poster_file_id UUID REFERENCES files (id) ON DELETE SET NULL,
    duration_secs  REAL,
    width          INT,
    height         INT,
    error          TEXT,
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);
