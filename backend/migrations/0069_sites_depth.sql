-- Websites: a site on its own domain (the reverse proxy sends that host to NERP), and
-- reusable groups of blocks to insert into any page. Analytics and the cookie banner live in
-- the site's settings.
ALTER TABLE site_sites ADD COLUMN domain TEXT UNIQUE;

CREATE TABLE site_saved_blocks (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name       TEXT NOT NULL,
    blocks     JSONB NOT NULL,
    created_by UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
