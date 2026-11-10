-- Drive: folders and files in a personal space per employee and a company space, with
-- versions, sharing, public links and a trash.

CREATE TABLE drive_nodes (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    -- folder | file
    kind       TEXT NOT NULL CHECK (kind IN ('folder', 'file')),
    -- personal (owner_id's) | company
    space      TEXT NOT NULL CHECK (space IN ('personal', 'company')),
    owner_id   UUID REFERENCES users (id) ON DELETE CASCADE,
    parent_id  UUID REFERENCES drive_nodes (id) ON DELETE CASCADE,
    name       TEXT NOT NULL,
    -- Files: the current version.
    file_id    UUID REFERENCES files (id) ON DELETE SET NULL,
    size_bytes BIGINT NOT NULL DEFAULT 0,
    mime_type  TEXT,
    version    INT NOT NULL DEFAULT 1,
    created_by UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_by UUID REFERENCES users (id) ON DELETE SET NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- In the trash since.
    deleted_at TIMESTAMPTZ,
    deleted_by UUID REFERENCES users (id) ON DELETE SET NULL,
    CHECK ((space = 'personal') = (owner_id IS NOT NULL))
);
CREATE INDEX drive_nodes_parent_idx ON drive_nodes (space ASC, owner_id ASC, parent_id ASC) WHERE deleted_at IS NULL;
CREATE INDEX drive_nodes_file_idx ON drive_nodes (file_id ASC);

CREATE TABLE drive_versions (
    node_id     UUID NOT NULL REFERENCES drive_nodes (id) ON DELETE CASCADE,
    version     INT NOT NULL,
    file_id     UUID REFERENCES files (id) ON DELETE SET NULL,
    size_bytes  BIGINT NOT NULL,
    uploaded_by UUID REFERENCES users (id) ON DELETE SET NULL,
    uploaded_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (node_id, version)
);
CREATE INDEX drive_versions_file_idx ON drive_versions (file_id ASC);

-- A folder or file shared from a personal space (folders share their contents).
CREATE TABLE drive_shares (
    node_id       UUID NOT NULL REFERENCES drive_nodes (id) ON DELETE CASCADE,
    user_id       UUID REFERENCES users (id) ON DELETE CASCADE,
    department_id UUID REFERENCES departments (id) ON DELETE CASCADE,
    -- view | edit
    level         TEXT NOT NULL CHECK (level IN ('view', 'edit')),
    CHECK ((user_id IS NULL) <> (department_id IS NULL))
);
CREATE INDEX drive_shares_node_idx ON drive_shares (node_id ASC);

-- Public download links for files.
CREATE TABLE drive_links (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    node_id    UUID NOT NULL REFERENCES drive_nodes (id) ON DELETE CASCADE,
    token_hash TEXT NOT NULL UNIQUE,
    expires_at TIMESTAMPTZ,
    downloads  INT NOT NULL DEFAULT 0,
    created_by UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
