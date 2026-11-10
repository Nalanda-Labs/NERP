-- Live chat: a chat button on the company's website (a channel), conversations with
-- visitors (sessions) answered by employees (the channel's operators), linked to CRM.

CREATE TABLE chat_channels (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name            TEXT NOT NULL,
    key             TEXT NOT NULL UNIQUE,
    -- Shown to visitors.
    title           TEXT NOT NULL DEFAULT 'Chat with us',
    greeting        TEXT NOT NULL DEFAULT 'Hi! How can we help?',
    color           TEXT NOT NULL DEFAULT '#1a73e8',
    -- Ask the visitor's name and email before the first message.
    ask_details     BOOLEAN NOT NULL DEFAULT TRUE,
    operators       UUID[] NOT NULL DEFAULT '{}',
    is_active       BOOLEAN NOT NULL DEFAULT TRUE,
    created_by      UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE chat_sessions (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    channel_id      UUID NOT NULL REFERENCES chat_channels (id) ON DELETE CASCADE,
    -- The visitor's browser holds the token; only its hash is stored.
    token_hash      TEXT NOT NULL UNIQUE,
    visitor_name    TEXT,
    visitor_email   TEXT,
    page_url        TEXT,
    -- open | closed
    status          TEXT NOT NULL DEFAULT 'open' CHECK (status IN ('open', 'closed')),
    assigned_to     UUID REFERENCES users (id) ON DELETE SET NULL,
    -- The CRM lead or contact the chat is about.
    item_id         UUID REFERENCES crm_items (id) ON DELETE SET NULL,
    -- Visitor messages the operators have not read yet.
    unread          INT NOT NULL DEFAULT 0,
    started_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_message_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    closed_at       TIMESTAMPTZ
);
CREATE INDEX chat_sessions_open_idx ON chat_sessions (status ASC, last_message_at DESC);
CREATE INDEX chat_sessions_item_idx ON chat_sessions (item_id ASC) WHERE item_id IS NOT NULL;

CREATE TABLE chat_messages (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    session_id UUID NOT NULL REFERENCES chat_sessions (id) ON DELETE CASCADE,
    -- visitor | operator | system
    author     TEXT NOT NULL CHECK (author IN ('visitor', 'operator', 'system')),
    user_id    UUID REFERENCES users (id) ON DELETE SET NULL,
    body       TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
);
CREATE INDEX chat_messages_session_idx ON chat_messages (session_id ASC, created_at ASC);
