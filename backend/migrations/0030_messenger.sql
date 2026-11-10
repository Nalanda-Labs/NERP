-- Messenger: one-to-one and group conversations between employees.

CREATE TABLE im_chats (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    -- direct | group
    kind            TEXT NOT NULL CHECK (kind IN ('direct', 'group')),
    title           TEXT,
    -- Direct chats: the two people's ids, sorted, so there is one chat per pair.
    direct_key      TEXT UNIQUE,
    created_by      UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_message_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE im_members (
    chat_id      UUID NOT NULL REFERENCES im_chats (id) ON DELETE CASCADE,
    user_id      UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    last_read_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    joined_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (chat_id, user_id)
);
CREATE INDEX im_members_user_idx ON im_members (user_id ASC);

CREATE TABLE im_messages (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    chat_id    UUID NOT NULL REFERENCES im_chats (id) ON DELETE CASCADE,
    -- NULL: a note from NERP ("Ann added Bob").
    user_id    UUID REFERENCES users (id) ON DELETE SET NULL,
    body       TEXT NOT NULL DEFAULT '',
    file_ids   UUID[] NOT NULL DEFAULT '{}',
    edited_at  TIMESTAMPTZ,
    deleted_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
);
CREATE INDEX im_messages_chat_idx ON im_messages (chat_id ASC, created_at DESC);
