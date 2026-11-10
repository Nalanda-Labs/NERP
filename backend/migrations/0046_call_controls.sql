-- The host of a call (whoever started it) can stop everyone else using their microphone,
-- camera or screen sharing, and mute everyone at once.
ALTER TABLE calls ADD COLUMN allow_audio BOOLEAN NOT NULL DEFAULT TRUE;
ALTER TABLE calls ADD COLUMN allow_video BOOLEAN NOT NULL DEFAULT TRUE;
ALTER TABLE calls ADD COLUMN allow_screen BOOLEAN NOT NULL DEFAULT TRUE;
CREATE INDEX call_invites_user_idx ON call_invites (user_id ASC);
