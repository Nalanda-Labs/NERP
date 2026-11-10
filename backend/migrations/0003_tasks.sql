-- Tasks & Projects.

CREATE TABLE projects (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name         TEXT NOT NULL,
    description  TEXT,
    -- open: every employee can see the project and its tasks; private: members only.
    visibility   TEXT NOT NULL DEFAULT 'private' CHECK (visibility IN ('open', 'private')),
    -- Accent colour for the project card/avatar.
    color        TEXT NOT NULL DEFAULT 'indigo',
    is_archived  BOOLEAN NOT NULL DEFAULT FALSE,
    created_by   UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE project_members (
    project_id  UUID NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    user_id     UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    -- owner: manages the project; member: works on tasks; viewer: read-only.
    role        TEXT NOT NULL CHECK (role IN ('owner', 'member', 'viewer')),
    added_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (project_id, user_id)
);
CREATE INDEX project_members_user_idx ON project_members (user_id);

-- Human-friendly task numbers (#123). CACHE keeps YugabyteDB from contending on every insert;
-- numbers stay unique but may have gaps.
CREATE SEQUENCE task_number_seq CACHE 100;

CREATE TABLE tasks (
    id                   UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    number               BIGINT NOT NULL DEFAULT nextval('task_number_seq'),
    title                TEXT NOT NULL,
    description          TEXT,
    status               TEXT NOT NULL DEFAULT 'new'
                         CHECK (status IN ('new', 'in_progress', 'review', 'done', 'deferred')),
    priority             TEXT NOT NULL DEFAULT 'normal' CHECK (priority IN ('low', 'normal', 'high')),
    project_id           UUID REFERENCES projects (id) ON DELETE SET NULL,
    parent_id            UUID REFERENCES tasks (id) ON DELETE SET NULL,
    created_by           UUID NOT NULL REFERENCES users (id),
    responsible_id       UUID NOT NULL REFERENCES users (id),
    deadline             TIMESTAMPTZ,
    -- When set, completion by anyone but the creator goes to 'review' for the creator to accept.
    requires_review      BOOLEAN NOT NULL DEFAULT FALSE,
    time_estimate_secs   INT,
    -- Ordering inside a board column (fractional so a card can be dropped between two others).
    position             DOUBLE PRECISION NOT NULL DEFAULT 0,
    completed_at         TIMESTAMPTZ,
    deadline_reminded_at TIMESTAMPTZ,
    overdue_notified_at  TIMESTAMPTZ,
    last_activity_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at           TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX tasks_number_uidx ON tasks (number);
CREATE INDEX tasks_responsible_idx ON tasks (responsible_id, status);
CREATE INDEX tasks_created_by_idx ON tasks (created_by, status);
CREATE INDEX tasks_project_idx ON tasks (project_id, status);
CREATE INDEX tasks_parent_idx ON tasks (parent_id);
CREATE INDEX tasks_deadline_idx ON tasks (status, deadline);

-- Co-assignees ("assistant") and watchers ("watcher"). Creator and responsible live on the task.
CREATE TABLE task_members (
    task_id   UUID NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
    user_id   UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    role      TEXT NOT NULL CHECK (role IN ('assistant', 'watcher')),
    PRIMARY KEY (task_id, user_id)
);
CREATE INDEX task_members_user_idx ON task_members (user_id, role);

CREATE TABLE task_checklist_items (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    task_id     UUID NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
    title       TEXT NOT NULL,
    is_done     BOOLEAN NOT NULL DEFAULT FALSE,
    sort        DOUBLE PRECISION NOT NULL DEFAULT 0,
    done_by     UUID REFERENCES users (id) ON DELETE SET NULL,
    done_at     TIMESTAMPTZ,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX task_checklist_task_idx ON task_checklist_items (task_id, sort);

CREATE TABLE task_comments (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    task_id     UUID NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
    author_id   UUID REFERENCES users (id) ON DELETE SET NULL,
    body        TEXT NOT NULL,
    edited_at   TIMESTAMPTZ,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX task_comments_task_idx ON task_comments (task_id, created_at);

CREATE TABLE task_files (
    task_id   UUID NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
    file_id   UUID NOT NULL REFERENCES files (id) ON DELETE CASCADE,
    added_by  UUID REFERENCES users (id) ON DELETE SET NULL,
    added_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (task_id, file_id)
);
CREATE INDEX task_files_file_idx ON task_files (file_id);

-- Every change to a task, for the History tab.
CREATE TABLE task_history (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    task_id     UUID NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
    actor_id    UUID REFERENCES users (id) ON DELETE SET NULL,
    field       TEXT NOT NULL,
    old_value   TEXT,
    new_value   TEXT,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX task_history_task_idx ON task_history (task_id, created_at);

-- Time tracking. A running timer has stopped_at NULL; each user has at most one.
CREATE TABLE task_time_entries (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    task_id     UUID NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
    user_id     UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    started_at  TIMESTAMPTZ NOT NULL,
    stopped_at  TIMESTAMPTZ,
    seconds     INT NOT NULL DEFAULT 0,
    comment     TEXT,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX task_time_task_idx ON task_time_entries (task_id, started_at);
CREATE UNIQUE INDEX task_time_running_uidx ON task_time_entries (user_id) WHERE stopped_at IS NULL;

-- Everyone may create projects by default; administrators may see and manage all tasks.
INSERT INTO role_permissions (role_id, permission)
    SELECT id, 'projects.create' FROM roles WHERE code = 'employee'
    ON CONFLICT DO NOTHING;
