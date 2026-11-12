-- Client portal: a contact signs in with a one-time link and sees their invoices, payments,
-- documents, signatures, tickets and bookings.
CREATE TABLE portal_access (
    contact_id    UUID PRIMARY KEY REFERENCES crm_items (id) ON DELETE CASCADE,
    enabled       BOOLEAN NOT NULL DEFAULT TRUE,
    -- Also the invoices and quotes of the contact's company (not only those they are on).
    company_wide  BOOLEAN NOT NULL DEFAULT FALSE,
    invited_by    UUID REFERENCES users (id) ON DELETE SET NULL,
    invited_at    TIMESTAMPTZ,
    last_seen_at  TIMESTAMPTZ
);

CREATE TABLE portal_links (
    hash        TEXT PRIMARY KEY,
    contact_id  UUID NOT NULL REFERENCES crm_items (id) ON DELETE CASCADE,
    expires_at  TIMESTAMPTZ NOT NULL,
    used_at     TIMESTAMPTZ
);

CREATE TABLE portal_sessions (
    hash          TEXT PRIMARY KEY,
    contact_id    UUID NOT NULL REFERENCES crm_items (id) ON DELETE CASCADE,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at    TIMESTAMPTZ NOT NULL,
    last_seen_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX portal_sessions_contact_idx ON portal_sessions (contact_id);
