-- Knowledge base: spaces of pages (a tree), written in Markdown, with every version kept.

CREATE TABLE kb_spaces (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name        TEXT NOT NULL,
    description TEXT,
    -- Who reads it: everyone, or the people and departments below.
    to_all      BOOLEAN NOT NULL DEFAULT TRUE,
    created_by  UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Readers (when not everyone) and editors.
CREATE TABLE kb_access (
    space_id      UUID NOT NULL REFERENCES kb_spaces (id) ON DELETE CASCADE,
    user_id       UUID REFERENCES users (id) ON DELETE CASCADE,
    department_id UUID REFERENCES departments (id) ON DELETE CASCADE,
    -- read | edit
    level         TEXT NOT NULL CHECK (level IN ('read', 'edit')),
    CHECK ((user_id IS NULL) <> (department_id IS NULL))
);
CREATE INDEX kb_access_space_idx ON kb_access (space_id ASC);

CREATE TABLE kb_pages (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    space_id   UUID NOT NULL REFERENCES kb_spaces (id) ON DELETE CASCADE,
    parent_id  UUID REFERENCES kb_pages (id) ON DELETE CASCADE,
    title      TEXT NOT NULL,
    body       TEXT NOT NULL DEFAULT '',
    sort       INT NOT NULL DEFAULT 0,
    version    INT NOT NULL DEFAULT 1,
    created_by UUID REFERENCES users (id) ON DELETE SET NULL,
    updated_by UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX kb_pages_space_idx ON kb_pages (space_id ASC, parent_id ASC, sort ASC);

-- Every saved version, for history and restoring.
CREATE TABLE kb_versions (
    page_id    UUID NOT NULL REFERENCES kb_pages (id) ON DELETE CASCADE,
    version    INT NOT NULL,
    title      TEXT NOT NULL,
    body       TEXT NOT NULL,
    saved_by   UUID REFERENCES users (id) ON DELETE SET NULL,
    saved_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (page_id, version)
);
