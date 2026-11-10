-- Calls through a LiveKit media server (when one is configured), and their recordings.

-- p2p: browsers connect to each other; livekit: everyone connects to the LiveKit server.
ALTER TABLE calls ADD COLUMN media TEXT NOT NULL DEFAULT 'p2p' CHECK (media IN ('p2p', 'livekit'));

CREATE TABLE call_recordings (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    call_id     UUID NOT NULL REFERENCES calls (id) ON DELETE CASCADE,
    -- LiveKit Egress's id for the recording.
    egress_id   TEXT,
    -- The file name Egress writes (in the recordings folder).
    file_name   TEXT NOT NULL,
    started_by  UUID REFERENCES users (id) ON DELETE SET NULL,
    started_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    stopped_at  TIMESTAMPTZ,
    -- recording | processing (Egress is finishing the file) | ready | failed
    status      TEXT NOT NULL DEFAULT 'recording' CHECK (status IN ('recording', 'processing', 'ready', 'failed')),
    file_id     UUID REFERENCES files (id) ON DELETE SET NULL,
    duration_secs INT,
    error       TEXT
);
CREATE INDEX call_recordings_call_idx ON call_recordings (call_id ASC);
CREATE INDEX call_recordings_open_idx ON call_recordings (status) WHERE status IN ('recording', 'processing');
