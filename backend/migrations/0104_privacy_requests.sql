-- Requests from people about their personal data: "what do you have on me" (access) and
-- "delete me" (erasure), entered by staff or sent through the public form (confirmed by email).
CREATE TABLE privacy_requests (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    kind         TEXT NOT NULL CHECK (kind IN ('access', 'erasure')),
    email        TEXT NOT NULL,
    phone        TEXT,
    name         TEXT,
    message      TEXT,
    source       TEXT NOT NULL CHECK (source IN ('staff', 'public')),
    status       TEXT NOT NULL CHECK (status IN ('unverified', 'open', 'done', 'rejected')),
    verify_hash  TEXT UNIQUE,
    verified_at  TIMESTAMPTZ,
    due_at       TIMESTAMPTZ,
    created_by   UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    decided_by   UUID REFERENCES users (id) ON DELETE SET NULL,
    decided_at   TIMESTAMPTZ,
    -- What was done, per source: counts and the action, never the data itself.
    outcome      JSONB,
    note         TEXT
);
CREATE INDEX privacy_requests_status_idx ON privacy_requests (status, due_at);

-- Who may handle them.
INSERT INTO role_permissions (role_id, permission)
SELECT id, 'privacy.manage' FROM roles WHERE code = 'admin'
ON CONFLICT DO NOTHING;
