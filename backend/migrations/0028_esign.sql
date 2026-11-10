-- E-signature: a generated CRM document sent to people who sign it through a link. The PDF
-- they sign is frozen when it is sent (its SHA-256 recorded); when everyone has signed, a
-- signed copy is made with a certificate page (signatures, times, addresses, fingerprint).

CREATE TABLE sign_requests (
    id               UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    document_id      UUID REFERENCES crm_documents (id) ON DELETE SET NULL,
    item_id          UUID NOT NULL REFERENCES crm_items (id) ON DELETE CASCADE,
    title            TEXT NOT NULL,
    message          TEXT,
    -- sent | completed | declined | cancelled | expired
    status           TEXT NOT NULL DEFAULT 'sent' CHECK (status IN ('sent', 'completed', 'declined', 'cancelled', 'expired')),
    original_file_id UUID REFERENCES files (id) ON DELETE SET NULL,
    original_sha256  TEXT NOT NULL,
    signed_file_id   UUID REFERENCES files (id) ON DELETE SET NULL,
    signed_sha256    TEXT,
    created_by       UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at       TIMESTAMPTZ NOT NULL DEFAULT now() + interval '30 days',
    completed_at     TIMESTAMPTZ
);
CREATE INDEX sign_requests_item_idx ON sign_requests (item_id ASC, created_at DESC);
CREATE INDEX sign_requests_document_idx ON sign_requests (document_id ASC);

CREATE TABLE sign_signers (
    id                UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    request_id        UUID NOT NULL REFERENCES sign_requests (id) ON DELETE CASCADE,
    name              TEXT NOT NULL,
    email             TEXT NOT NULL,
    sort              INT NOT NULL DEFAULT 0,
    token_hash        TEXT NOT NULL UNIQUE,
    -- pending | viewed | signed | declined
    status            TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'viewed', 'signed', 'declined')),
    viewed_at         TIMESTAMPTZ,
    signed_at         TIMESTAMPTZ,
    signed_name       TEXT,
    signature_file_id UUID REFERENCES files (id) ON DELETE SET NULL,
    ip                TEXT,
    user_agent        TEXT,
    decline_reason    TEXT
);
CREATE INDEX sign_signers_request_idx ON sign_signers (request_id ASC, sort ASC);
