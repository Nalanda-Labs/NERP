-- Sales intelligence: visits to the company's websites (through a tracking script), each
-- visitor's journey to the CRM, traffic analysis with ad spend, and short links for offline
-- ads (posters, print, radio) that count who came from them.

-- A website carrying the tracking script.
CREATE TABLE track_sites (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name        TEXT NOT NULL,
    -- Host names the script may report from (empty: any); "www." is ignored.
    domains     TEXT[] NOT NULL DEFAULT '{}',
    key         TEXT NOT NULL UNIQUE,
    is_active   BOOLEAN NOT NULL DEFAULT TRUE,
    created_by  UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- A browser, by the id the script keeps in it; linked to the CRM record it became.
CREATE TABLE track_visitors (
    id          UUID PRIMARY KEY,
    item_id     UUID REFERENCES crm_items (id) ON DELETE SET NULL,
    first_seen  TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen   TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX track_visitors_item_idx ON track_visitors (item_id) WHERE item_id IS NOT NULL;

-- A short link (also as a QR code) for an ad: it redirects to the target with UTM tags.
CREATE TABLE track_links (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    code        TEXT NOT NULL UNIQUE,
    name        TEXT NOT NULL,
    target_url  TEXT NOT NULL,
    -- The traffic source visits through it count for (a CRM traffic source's code, or free text).
    source      TEXT NOT NULL,
    -- offline (posters, print, radio, events) or online.
    kind        TEXT NOT NULL DEFAULT 'offline' CHECK (kind IN ('offline', 'online')),
    utm         JSONB NOT NULL DEFAULT '{}',
    clicks      BIGINT NOT NULL DEFAULT 0,
    is_active   BOOLEAN NOT NULL DEFAULT TRUE,
    created_by  UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- One visit: pages seen within 30 minutes of each other, from one source.
CREATE TABLE track_visits (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    visitor_id  UUID NOT NULL REFERENCES track_visitors (id) ON DELETE CASCADE,
    site_id     UUID NOT NULL REFERENCES track_sites (id) ON DELETE CASCADE,
    started_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- paid | search | social | email | referral | campaign | offline | direct
    channel     TEXT NOT NULL,
    source      TEXT NOT NULL,
    utm         JSONB NOT NULL DEFAULT '{}',
    referrer    TEXT,
    landing     TEXT NOT NULL,
    pages       INT NOT NULL DEFAULT 0,
    link_id     UUID REFERENCES track_links (id) ON DELETE SET NULL
);
CREATE INDEX track_visits_visitor_idx ON track_visits (visitor_id, started_at DESC);
CREATE INDEX track_visits_time_idx ON track_visits (started_at);

CREATE TABLE track_pages (
    visit_id    UUID NOT NULL REFERENCES track_visits (id) ON DELETE CASCADE,
    at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    url         TEXT NOT NULL,
    title       TEXT
);
CREATE INDEX track_pages_visit_idx ON track_pages (visit_id, at);

-- What was spent on a channel or source in a month (in the base currency, as entered then).
CREATE TABLE track_spend (
    source      TEXT NOT NULL,
    month       DATE NOT NULL CHECK (extract(day FROM month) = 1),
    amount      NUMERIC(24, 4) NOT NULL CHECK (amount >= 0),
    currency    CHAR(3) NOT NULL,
    amount_base NUMERIC(24, 4) NOT NULL,
    note        TEXT,
    updated_by  UUID REFERENCES users (id) ON DELETE SET NULL,
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (source, month)
);

-- The visitor a live chat came from (linked to its CRM record once there is one).
ALTER TABLE chat_sessions ADD COLUMN visitor_id UUID;
