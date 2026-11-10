-- At most one recording going on per call (several people pressing Record at once).
CREATE UNIQUE INDEX call_recordings_one_active_idx ON call_recordings (call_id) WHERE status = 'recording';
