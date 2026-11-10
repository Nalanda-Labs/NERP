-- Website builder: sites made of pages made of blocks, drafts and published versions,
-- served at /s/<site>/<page>, with daily page views.

CREATE TABLE site_sites (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name       TEXT NOT NULL,
    slug       TEXT NOT NULL UNIQUE,
    -- {"color": "#1a73e8", "font": "sans|serif", "logo_file_id": "…", "footer": "…"}
    settings   JSONB NOT NULL DEFAULT '{}',
    created_by UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE site_pages (
    id                    UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    site_id               UUID NOT NULL REFERENCES site_sites (id) ON DELETE CASCADE,
    -- '' is the home page.
    path                  TEXT NOT NULL,
    title                 TEXT NOT NULL,
    description           TEXT,
    blocks                JSONB NOT NULL DEFAULT '[]',
    in_menu               BOOLEAN NOT NULL DEFAULT TRUE,
    sort                  INT NOT NULL DEFAULT 0,
    -- What visitors see; NULL until published.
    published_title       TEXT,
    published_description TEXT,
    published_blocks      JSONB,
    published_at          TIMESTAMPTZ,
    updated_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (site_id, path)
);

CREATE TABLE site_views (
    page_id UUID NOT NULL REFERENCES site_pages (id) ON DELETE CASCADE,
    day     DATE NOT NULL,
    views   INT NOT NULL DEFAULT 0,
    PRIMARY KEY (page_id, day)
);
