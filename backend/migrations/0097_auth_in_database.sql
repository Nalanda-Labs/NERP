-- Sessions, used refresh tokens and emailed link tokens in PostgreSQL (Redis keeps a cache),
-- so losing Redis signs nobody out and keeps password-reset and invitation links working.
CREATE TABLE auth_sessions (
    id           UUID PRIMARY KEY,
    user_id      UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    refresh_hash TEXT NOT NULL UNIQUE,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_used_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    ip           TEXT,
    user_agent   TEXT,
    expires_at   TIMESTAMPTZ NOT NULL
);
CREATE INDEX auth_sessions_user ON auth_sessions (user_id);
CREATE INDEX auth_sessions_expires ON auth_sessions (expires_at);

-- Refresh tokens already exchanged: seeing one again means a copy was stolen.
CREATE TABLE auth_used_refresh (
    hash       TEXT PRIMARY KEY,
    session_id UUID NOT NULL,
    used_at    TIMESTAMPTZ NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL
);
CREATE INDEX auth_used_refresh_expires ON auth_used_refresh (expires_at);

CREATE TABLE auth_link_tokens (
    hash       TEXT PRIMARY KEY,
    scope      TEXT NOT NULL,
    owner_id   UUID NOT NULL,
    value      JSONB NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL
);
CREATE INDEX auth_link_tokens_owner ON auth_link_tokens (scope, owner_id);
