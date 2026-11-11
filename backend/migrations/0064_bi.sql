-- BI: dashboards of charts built from datasets. Datasets are NERP's own data (built in, in
-- code), uploaded CSV files, CSV or JSON fetched from a URL, or tables and queries in an
-- outside PostgreSQL or MySQL database. Dashboards are shared with people, departments or
-- everyone, or through public links; saved filter presets per dashboard. People with the
-- bi.manage permission build and change dashboards and datasets; others view what is
-- shared with them.

-- An outside database.
CREATE TABLE bi_sources (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name        TEXT NOT NULL,
    kind        TEXT NOT NULL CHECK (kind IN ('postgres', 'mysql')),
    host        TEXT NOT NULL,
    port        INT NOT NULL,
    database    TEXT NOT NULL,
    username    TEXT NOT NULL,
    -- Sealed with the data key.
    password    BYTEA,
    tls         BOOLEAN NOT NULL DEFAULT FALSE,
    created_by  UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE bi_datasets (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name          TEXT NOT NULL,
    -- csv: uploaded; url: fetched (CSV or JSON) and refreshed; sql: a query on a source.
    kind          TEXT NOT NULL CHECK (kind IN ('csv', 'url', 'sql')),
    source_id     UUID REFERENCES bi_sources (id) ON DELETE CASCADE,
    query         TEXT,
    url           TEXT,
    refresh_hours INT,
    -- [{"key", "label", "type": text|number|date|bool}]
    columns       JSONB NOT NULL DEFAULT '[]',
    row_count     INT NOT NULL DEFAULT 0,
    refreshed_at  TIMESTAMPTZ,
    last_error    TEXT,
    created_by    UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK ((kind = 'sql') = (source_id IS NOT NULL))
);

-- Rows of csv and url datasets; values are normalised (numbers as numbers, dates as ISO
-- timestamps, anything unreadable as null).
CREATE TABLE bi_rows (
    dataset_id UUID NOT NULL REFERENCES bi_datasets (id) ON DELETE CASCADE,
    n          INT NOT NULL,
    data       JSONB NOT NULL,
    PRIMARY KEY (dataset_id, n)
);

CREATE TABLE bi_dashboards (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    title       TEXT NOT NULL,
    description TEXT,
    owner_id    UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    -- Default filters: {"period": {...}, "filters": [...]}
    filters     JSONB NOT NULL DEFAULT '{}',
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX bi_dashboards_owner_idx ON bi_dashboards (owner_id ASC);

CREATE TABLE bi_widgets (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    dashboard_id UUID NOT NULL REFERENCES bi_dashboards (id) ON DELETE CASCADE,
    position     INT NOT NULL DEFAULT 0,
    title        TEXT NOT NULL,
    -- number, bar, hbar, line, area, pie, funnel, table
    chart        TEXT NOT NULL,
    -- Width in twelfths and height in rows.
    w            SMALLINT NOT NULL DEFAULT 6 CHECK (w BETWEEN 2 AND 12),
    h            SMALLINT NOT NULL DEFAULT 2 CHECK (h BETWEEN 1 AND 6),
    -- The query: dataset, groups, measures, filters, sort, limit, date field.
    spec         JSONB NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX bi_widgets_dashboard_idx ON bi_widgets (dashboard_id ASC, position ASC);

CREATE TABLE bi_access (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    dashboard_id UUID NOT NULL REFERENCES bi_dashboards (id) ON DELETE CASCADE,
    grantee_kind TEXT NOT NULL CHECK (grantee_kind IN ('everyone', 'department', 'user')),
    grantee_id   UUID,
    CHECK ((grantee_kind = 'everyone') = (grantee_id IS NULL))
);
CREATE UNIQUE INDEX bi_access_one_idx ON bi_access (dashboard_id, grantee_kind, COALESCE(grantee_id, '00000000-0000-0000-0000-000000000000'::uuid));

-- Public (view-only) links; only the hash of the token is kept.
CREATE TABLE bi_links (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    dashboard_id  UUID NOT NULL REFERENCES bi_dashboards (id) ON DELETE CASCADE,
    token_hash    TEXT NOT NULL UNIQUE,
    expires_at    TIMESTAMPTZ,
    password_hash TEXT,
    created_by    UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Saved filter values: one's own, or shared with everyone who opens the dashboard.
CREATE TABLE bi_presets (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    dashboard_id UUID NOT NULL REFERENCES bi_dashboards (id) ON DELETE CASCADE,
    user_id      UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    name         TEXT NOT NULL,
    shared       BOOLEAN NOT NULL DEFAULT FALSE,
    filters      JSONB NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX bi_presets_dashboard_idx ON bi_presets (dashboard_id ASC);

-- Helpers for the built-in datasets.
CREATE FUNCTION bi_person(u UUID) RETURNS TEXT LANGUAGE sql STABLE AS $$
    SELECT NULLIF(trim(first_name || ' ' || last_name), '') FROM users WHERE id = u
$$;
CREATE FUNCTION bi_department(u UUID) RETURNS TEXT LANGUAGE sql STABLE AS $$
    SELECT d.name FROM user_departments ud JOIN departments d ON d.id = ud.department_id WHERE ud.user_id = u ORDER BY d.sort, d.name LIMIT 1
$$;
