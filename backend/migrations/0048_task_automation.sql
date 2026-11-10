-- Automation on a project's board: rules that act when a task enters a column, and
-- triggers that move a task to a column when something happens to it. Task templates can
-- hold subtasks and be shared with everyone.

CREATE TABLE project_stage_rules (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    project_id UUID NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    stage_id   UUID NOT NULL REFERENCES project_stages (id) ON DELETE CASCADE,
    -- notify | comment | set_responsible | add_assistant | add_watcher | set_deadline |
    -- set_priority | set_status | create_subtask
    kind       TEXT NOT NULL,
    params     JSONB NOT NULL DEFAULT '{}',
    sort       INT NOT NULL DEFAULT 0,
    created_by UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX project_stage_rules_stage_idx ON project_stage_rules (stage_id ASC, sort ASC);

CREATE TABLE project_stage_triggers (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    project_id UUID NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    stage_id   UUID NOT NULL REFERENCES project_stages (id) ON DELETE CASCADE,
    -- comment: someone comments; overdue: the deadline passes; checklist_done: every
    -- checklist point is ticked; started: work starts on it.
    event      TEXT NOT NULL CHECK (event IN ('comment', 'overdue', 'checklist_done', 'started')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (project_id, event)
);

-- What each rule did, for the board's automation log.
CREATE TABLE project_automation_log (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    project_id UUID NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    task_id    UUID REFERENCES tasks (id) ON DELETE SET NULL,
    what       TEXT NOT NULL,
    ok         BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX project_automation_log_idx ON project_automation_log (project_id ASC, created_at DESC);

-- Subtasks a template makes along with the task: [{title, responsible_id?, deadline_offset_secs?}].
ALTER TABLE task_templates ADD COLUMN subtasks JSONB NOT NULL DEFAULT '[]';
-- Everyone may see and use it (only its owner changes it).
ALTER TABLE task_templates ADD COLUMN shared BOOLEAN NOT NULL DEFAULT FALSE;
