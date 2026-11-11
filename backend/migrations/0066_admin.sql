-- Security and administration: links anyone can use to ask to join the company (optionally
-- only from some email domains, a number of times, until a date, and with an administrator's
-- approval), people waiting for that approval, and who joined through which link.
-- Company-wide settings (IP address ranges, public links, chat retention) live in `settings`.

CREATE TABLE join_links (
    id             UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    -- Only the hash of the token is kept.
    token_hash     TEXT NOT NULL UNIQUE,
    -- Shown to the administrator ("Sales team, spring 2027").
    note           TEXT,
    department_id  UUID REFERENCES departments (id) ON DELETE SET NULL,
    -- Lower case, without "@"; empty: any address.
    email_domains  TEXT[] NOT NULL DEFAULT '{}',
    needs_approval BOOLEAN NOT NULL DEFAULT TRUE,
    max_uses       INT CHECK (max_uses IS NULL OR max_uses > 0),
    uses           INT NOT NULL DEFAULT 0,
    expires_at     TIMESTAMPTZ,
    disabled       BOOLEAN NOT NULL DEFAULT FALSE,
    created_by     UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Waiting for an administrator (inactive until approved).
ALTER TABLE users ADD COLUMN join_pending BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE users ADD COLUMN joined_via UUID REFERENCES join_links (id) ON DELETE SET NULL;
CREATE INDEX users_join_pending_idx ON users (created_at DESC) WHERE join_pending;
