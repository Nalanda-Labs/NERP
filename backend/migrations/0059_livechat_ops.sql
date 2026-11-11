-- Live chat operations: how chats are handed out (in turn, or offered to everyone; at most so
-- many each; passed on when unanswered), working hours with an away message, automatic
-- replies (after the first message, when nobody answers, on closing; closing idle chats),
-- consent before chatting, the visitor's rating, a supervisor's review, saved replies,
-- passing a chat to a colleague or another channel, and figures.

ALTER TABLE chat_channels ADD COLUMN distribution TEXT NOT NULL DEFAULT 'queue' CHECK (distribution IN ('queue', 'all'));
-- At most this many open chats per operator (NULL: no limit); those over it wait.
ALTER TABLE chat_channels ADD COLUMN max_chats INT CHECK (max_chats IS NULL OR max_chats BETWEEN 1 AND 100);
-- Unanswered for this long, a chat goes to the next operator.
ALTER TABLE chat_channels ADD COLUMN requeue_minutes INT CHECK (requeue_minutes IS NULL OR requeue_minutes BETWEEN 1 AND 240);
-- [[weekday 1-7, from minute, to minute], …] in `timezone`; NULL: always open.
ALTER TABLE chat_channels ADD COLUMN hours JSONB;
ALTER TABLE chat_channels ADD COLUMN timezone TEXT NOT NULL DEFAULT 'UTC';
ALTER TABLE chat_channels ADD COLUMN offline_message TEXT;
ALTER TABLE chat_channels ADD COLUMN auto_reply TEXT;
ALTER TABLE chat_channels ADD COLUMN no_answer_minutes INT CHECK (no_answer_minutes IS NULL OR no_answer_minutes BETWEEN 1 AND 240);
ALTER TABLE chat_channels ADD COLUMN no_answer_message TEXT;
ALTER TABLE chat_channels ADD COLUMN close_message TEXT;
ALTER TABLE chat_channels ADD COLUMN idle_close_minutes INT CHECK (idle_close_minutes IS NULL OR idle_close_minutes BETWEEN 5 AND 10080);
-- Visitors agree to this before chatting (e.g. the privacy notice).
ALTER TABLE chat_channels ADD COLUMN consent_text TEXT;
ALTER TABLE chat_channels ADD COLUMN ask_rating BOOLEAN NOT NULL DEFAULT TRUE;

ALTER TABLE chat_sessions ADD COLUMN consent_at TIMESTAMPTZ;
-- When the current operator got it (for passing it on), and the first answer.
ALTER TABLE chat_sessions ADD COLUMN offered_at TIMESTAMPTZ;
ALTER TABLE chat_sessions ADD COLUMN first_reply_at TIMESTAMPTZ;
ALTER TABLE chat_sessions ADD COLUMN no_answer_sent BOOLEAN NOT NULL DEFAULT FALSE;
-- Started outside working hours.
ALTER TABLE chat_sessions ADD COLUMN offline BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE chat_sessions ADD COLUMN rating SMALLINT CHECK (rating IS NULL OR rating BETWEEN 1 AND 5);
ALTER TABLE chat_sessions ADD COLUMN rating_comment TEXT;
ALTER TABLE chat_sessions ADD COLUMN review_score SMALLINT CHECK (review_score IS NULL OR review_score BETWEEN 1 AND 5);
ALTER TABLE chat_sessions ADD COLUMN review_comment TEXT;
ALTER TABLE chat_sessions ADD COLUMN reviewed_by UUID REFERENCES users (id) ON DELETE SET NULL;
ALTER TABLE chat_sessions ADD COLUMN reviewed_at TIMESTAMPTZ;
CREATE INDEX chat_sessions_started_idx ON chat_sessions (started_at DESC);

-- Saved replies: for one channel or all, with a short code to type ("/hours").
CREATE TABLE chat_canned (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    channel_id UUID REFERENCES chat_channels (id) ON DELETE CASCADE,
    shortcut   TEXT NOT NULL,
    title      TEXT NOT NULL,
    body       TEXT NOT NULL,
    created_by UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX chat_canned_shortcut_idx ON chat_canned (COALESCE(channel_id, '00000000-0000-0000-0000-000000000000'::uuid), lower(shortcut));
