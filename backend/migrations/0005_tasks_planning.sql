-- Planning features: custom board columns, Gantt dates and dependencies, templates and recurrence.

-- ---------- Board columns (per project) ----------
CREATE TABLE project_stages (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    project_id  UUID NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    name        TEXT NOT NULL,
    color       TEXT NOT NULL DEFAULT 'slate',
    sort        DOUBLE PRECISION NOT NULL DEFAULT 0,
    -- Optional link to a task status: dropping a card here applies that status, and a task
    -- whose status changes elsewhere moves to the first column linked to its new status.
    status      TEXT CHECK (status IN ('new', 'in_progress', 'review', 'done', 'deferred')),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX project_stages_project_idx ON project_stages (project_id, sort);

-- ---------- Templates & recurrence ----------
CREATE TABLE task_templates (
    id                    UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    title                 TEXT NOT NULL,
    description           TEXT,
    -- NULL: the person using the template is responsible.
    responsible_id        UUID REFERENCES users (id) ON DELETE SET NULL,
    assistant_ids         UUID[] NOT NULL DEFAULT '{}',
    watcher_ids           UUID[] NOT NULL DEFAULT '{}',
    priority              TEXT NOT NULL DEFAULT 'normal' CHECK (priority IN ('low', 'normal', 'high')),
    project_id            UUID REFERENCES projects (id) ON DELETE SET NULL,
    requires_review       BOOLEAN NOT NULL DEFAULT FALSE,
    time_estimate_secs    INT,
    checklist             TEXT[] NOT NULL DEFAULT '{}',
    -- Deadline relative to when a task is created from the template.
    deadline_offset_secs  INT,
    -- Schedule for recurring creation (see tasks::recurrence); NULL for a plain template.
    recurrence            JSONB,
    next_run_at           TIMESTAMPTZ,
    last_run_at           TIMESTAMPTZ,
    run_count             INT NOT NULL DEFAULT 0,
    is_active             BOOLEAN NOT NULL DEFAULT TRUE,
    created_by            UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    created_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at            TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX task_templates_owner_idx ON task_templates (created_by);
CREATE INDEX task_templates_due_idx ON task_templates (is_active, next_run_at);

-- ---------- Task columns ----------
ALTER TABLE tasks ADD COLUMN start_date TIMESTAMPTZ;
ALTER TABLE tasks ADD COLUMN stage_id UUID REFERENCES project_stages (id) ON DELETE SET NULL;
ALTER TABLE tasks ADD COLUMN template_id UUID REFERENCES task_templates (id) ON DELETE SET NULL;
CREATE INDEX tasks_stage_idx ON tasks (stage_id);

-- ---------- Dependencies ("task_id is blocked by depends_on_id", finish-to-start) ----------
CREATE TABLE task_dependencies (
    task_id        UUID NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
    depends_on_id  UUID NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
    created_by     UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (task_id, depends_on_id),
    CHECK (task_id <> depends_on_id)
);
CREATE INDEX task_dependencies_rev_idx ON task_dependencies (depends_on_id);

-- ---------- Default columns for existing projects ----------
INSERT INTO project_stages (project_id, name, color, sort, status) SELECT id, 'To do', 'slate', 1, 'new' FROM projects;
INSERT INTO project_stages (project_id, name, color, sort, status) SELECT id, 'In progress', 'sky', 2, 'in_progress' FROM projects;
INSERT INTO project_stages (project_id, name, color, sort, status) SELECT id, 'Review', 'amber', 3, 'review' FROM projects;
INSERT INTO project_stages (project_id, name, color, sort, status) SELECT id, 'Done', 'emerald', 4, 'done' FROM projects;
UPDATE tasks t SET stage_id = (
    SELECT s.id FROM project_stages s
    WHERE s.project_id = t.project_id
      AND s.status = CASE WHEN t.status = 'deferred' THEN 'new' ELSE t.status END
    ORDER BY s.sort LIMIT 1)
WHERE t.project_id IS NOT NULL;
