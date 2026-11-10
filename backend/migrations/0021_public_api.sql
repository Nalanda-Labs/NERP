-- Public API: personal API keys and outgoing event webhooks.

-- A key acts as its owner, limited to its scopes ("crm", "crm:read", "tasks", "tasks:read",
-- "users:read", "files"). Only a hash of the secret is stored; the prefix identifies it in lists.
CREATE TABLE api_keys (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id       UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    name          TEXT NOT NULL,
    prefix        TEXT NOT NULL,
    secret_hash   TEXT NOT NULL UNIQUE,
    scopes        TEXT[] NOT NULL,
    expires_at    TIMESTAMPTZ,
    last_used_at  TIMESTAMPTZ,
    last_used_ip  TEXT,
    revoked_at    TIMESTAMPTZ,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX api_keys_user_idx ON api_keys (user_id ASC, created_at DESC);

-- Outgoing webhooks: events are POSTed (signed) to the subscriber's URL.
CREATE TABLE webhook_subscriptions (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name          TEXT NOT NULL,
    url           TEXT NOT NULL,
    secret        TEXT NOT NULL,
    events        TEXT[] NOT NULL,
    is_active     BOOLEAN NOT NULL DEFAULT TRUE,
    created_by    UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE webhook_deliveries (
    id               UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    subscription_id  UUID NOT NULL REFERENCES webhook_subscriptions (id) ON DELETE CASCADE,
    event            TEXT NOT NULL,
    payload          JSONB NOT NULL,
    -- pending | done | failed
    status           TEXT NOT NULL DEFAULT 'pending',
    attempts         INT NOT NULL DEFAULT 0,
    response_status  INT,
    response_body    TEXT,
    error            TEXT,
    next_attempt_at  TIMESTAMPTZ,
    delivered_at     TIMESTAMPTZ,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX webhook_deliveries_sub_idx ON webhook_deliveries (subscription_id ASC, created_at DESC);
