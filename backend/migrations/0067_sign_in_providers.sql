-- Signing in with other accounts: Google, Microsoft, Apple, Facebook, X, or any OpenID
-- Connect provider, next to the password and the company sign-on. Only existing employees
-- sign in this way: matched by an email address the provider has verified, or linked from
-- their own profile first. Secrets are sealed with the data key (for Apple: the private key
-- that signs the client secret).

CREATE TABLE sign_in_providers (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    kind       TEXT NOT NULL CHECK (kind IN ('google', 'microsoft', 'apple', 'facebook', 'x', 'oidc')),
    label      TEXT NOT NULL,
    client_id  TEXT NOT NULL,
    secret     BYTEA,
    -- OpenID Connect: the issuer; Apple: the team and key ids.
    issuer     TEXT,
    team_id    TEXT,
    key_id     TEXT,
    enabled    BOOLEAN NOT NULL DEFAULT TRUE,
    position   INT NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Who an outside account is.
CREATE TABLE user_identities (
    provider_id  UUID NOT NULL REFERENCES sign_in_providers (id) ON DELETE CASCADE,
    subject      TEXT NOT NULL,
    user_id      UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    email        TEXT,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_used_at TIMESTAMPTZ,
    PRIMARY KEY (provider_id, subject)
);
CREATE INDEX user_identities_user_idx ON user_identities (user_id ASC);
