-- Marketing: segments (saved audiences), email campaigns with per-recipient tracking, and
-- the opt-out list every campaign honours.

CREATE TABLE mkt_segments (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name         TEXT NOT NULL,
    entity_type  SMALLINT NOT NULL REFERENCES crm_entity_types (id) ON DELETE CASCADE,
    -- The same filters as the CRM lists (q, semantics, source, created_from, uf, …).
    filter       JSONB NOT NULL DEFAULT '{}',
    created_by   UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE mkt_campaigns (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name         TEXT NOT NULL,
    -- Goes into links as utm_campaign, which ties leads and deals back to the campaign.
    slug         TEXT NOT NULL UNIQUE,
    -- draft | scheduled | sending | sent | cancelled
    status       TEXT NOT NULL DEFAULT 'draft',
    segment_ids  UUID[] NOT NULL DEFAULT '{}',
    subject      TEXT NOT NULL DEFAULT '',
    body         TEXT NOT NULL DEFAULT '',
    from_name    TEXT,
    reply_to     TEXT,
    send_at      TIMESTAMPTZ,
    started_at   TIMESTAMPTZ,
    finished_at  TIMESTAMPTZ,
    -- Records left out when sending started (no email address, unsubscribed, duplicates).
    skipped      INT NOT NULL DEFAULT 0,
    created_by   UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX mkt_campaigns_due_idx ON mkt_campaigns (send_at ASC) WHERE status = 'scheduled';

CREATE TABLE mkt_recipients (
    id               UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    campaign_id      UUID NOT NULL REFERENCES mkt_campaigns (id) ON DELETE CASCADE,
    item_id          UUID REFERENCES crm_items (id) ON DELETE SET NULL,
    email            TEXT NOT NULL,
    -- Secret in tracking and unsubscribe links.
    token            TEXT NOT NULL UNIQUE,
    -- pending | sent | failed
    status           TEXT NOT NULL DEFAULT 'pending',
    error            TEXT,
    sent_at          TIMESTAMPTZ,
    opened_at        TIMESTAMPTZ,
    open_count       INT NOT NULL DEFAULT 0,
    clicked_at       TIMESTAMPTZ,
    click_count      INT NOT NULL DEFAULT 0,
    unsubscribed_at  TIMESTAMPTZ
);
CREATE INDEX mkt_recipients_campaign_idx ON mkt_recipients (campaign_id ASC, status ASC);

CREATE TABLE mkt_clicks (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    recipient_id  UUID NOT NULL REFERENCES mkt_recipients (id) ON DELETE CASCADE,
    url           TEXT NOT NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX mkt_clicks_recipient_idx ON mkt_clicks (recipient_id ASC);

-- Addresses that asked not to get marketing email (lower case).
CREATE TABLE mkt_optouts (
    email        TEXT PRIMARY KEY,
    campaign_id  UUID REFERENCES mkt_campaigns (id) ON DELETE SET NULL,
    -- unsubscribed (by the person) | added (by an employee)
    source       TEXT NOT NULL DEFAULT 'unsubscribed',
    created_by   UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);
