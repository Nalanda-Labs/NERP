-- Whiteboards: an Excalidraw drawing several people edit at once (changes are merged element
-- by element), with who may see or edit it, public links (view or edit, optionally with an
-- expiry date and a password), images kept as NERP files, a chat to discuss it, and boards
-- made inside a task or a workgroup. Templates are boards marked as such.

CREATE TABLE boards (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    owner_id    UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    title       TEXT NOT NULL,
    -- {"elements": [...], "appState": {...}} (Excalidraw's scene, without image data).
    scene       JSONB NOT NULL DEFAULT '{"elements": [], "appState": {}}',
    -- Raised on every saved change (clients ask for what's newer than theirs).
    version     BIGINT NOT NULL DEFAULT 0,
    is_template BOOLEAN NOT NULL DEFAULT FALSE,
    -- Made inside a task or a workgroup (people there may open it).
    task_id     UUID REFERENCES tasks (id) ON DELETE SET NULL,
    project_id  UUID REFERENCES projects (id) ON DELETE SET NULL,
    -- Read-only to all but its owner.
    locked      BOOLEAN NOT NULL DEFAULT FALSE,
    updated_by  UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX boards_owner_idx ON boards (owner_id ASC, updated_at DESC);
CREATE INDEX boards_task_idx ON boards (task_id ASC) WHERE task_id IS NOT NULL;
CREATE INDEX boards_project_idx ON boards (project_id ASC) WHERE project_id IS NOT NULL;

-- Who else may open it: everyone, departments, people; view or edit.
CREATE TABLE board_access (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    board_id     UUID NOT NULL REFERENCES boards (id) ON DELETE CASCADE,
    grantee_kind TEXT NOT NULL CHECK (grantee_kind IN ('everyone', 'department', 'user')),
    grantee_id   UUID,
    level        TEXT NOT NULL CHECK (level IN ('view', 'edit')),
    CHECK ((grantee_kind = 'everyone') = (grantee_id IS NULL))
);
CREATE UNIQUE INDEX board_access_one_idx ON board_access (board_id, grantee_kind, COALESCE(grantee_id, '00000000-0000-0000-0000-000000000000'::uuid));

-- Public links (only the hash of the token is kept).
CREATE TABLE board_links (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    board_id      UUID NOT NULL REFERENCES boards (id) ON DELETE CASCADE,
    token_hash    TEXT NOT NULL UNIQUE,
    level         TEXT NOT NULL CHECK (level IN ('view', 'edit')),
    expires_at    TIMESTAMPTZ,
    password_hash TEXT,
    created_by    UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Images on a board: Excalidraw's file id → a NERP file.
CREATE TABLE board_files (
    board_id UUID NOT NULL REFERENCES boards (id) ON DELETE CASCADE,
    file_key TEXT NOT NULL,
    file_id  UUID NOT NULL REFERENCES files (id) ON DELETE CASCADE,
    PRIMARY KEY (board_id, file_key)
);

-- A board's chat.
ALTER TABLE im_chats ADD COLUMN board_id UUID UNIQUE REFERENCES boards (id) ON DELETE CASCADE;
