-- Task depth: who does each checklist point, rating finished tasks, a result (a comment marked
-- as the outcome), new comments since I last looked, a personal planner, custom fields,
-- deadlines that skip weekends and holidays, and a recycle bin.

ALTER TABLE task_checklist_items ADD COLUMN assignee_id UUID REFERENCES users (id) ON DELETE SET NULL;

-- The creator's verdict on a finished task.
ALTER TABLE tasks ADD COLUMN rating TEXT CHECK (rating IN ('good', 'bad'));
ALTER TABLE tasks ADD COLUMN rated_at TIMESTAMPTZ;
-- The comment that states the task's result; with requires_result the task can't be finished
-- without one.
ALTER TABLE tasks ADD COLUMN result_comment_id UUID REFERENCES task_comments (id) ON DELETE SET NULL;
ALTER TABLE tasks ADD COLUMN requires_result BOOLEAN NOT NULL DEFAULT FALSE;
-- Deadlines (and start dates) move off weekends and company holidays.
ALTER TABLE tasks ADD COLUMN skip_nonworking BOOLEAN NOT NULL DEFAULT FALSE;

-- When each person last opened each task: comments after that are new to them.
CREATE TABLE task_views (
    task_id   UUID NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
    user_id   UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    viewed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (task_id, user_id)
);
CREATE INDEX task_views_user_idx ON task_views (user_id ASC);

-- Each person's planner: their own columns for the tasks they work on.
CREATE TABLE task_planner_stages (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id    UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    name       TEXT NOT NULL,
    color      TEXT NOT NULL DEFAULT 'slate',
    sort       DOUBLE PRECISION NOT NULL DEFAULT 0
);
CREATE INDEX task_planner_stages_user_idx ON task_planner_stages (user_id ASC, sort ASC);

CREATE TABLE task_planner (
    user_id  UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    task_id  UUID NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
    stage_id UUID NOT NULL REFERENCES task_planner_stages (id) ON DELETE CASCADE,
    position DOUBLE PRECISION NOT NULL DEFAULT 0,
    PRIMARY KEY (user_id, task_id)
);

-- Custom fields: for every task, or only for one project's tasks.
CREATE TABLE task_field_defs (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    project_id UUID REFERENCES projects (id) ON DELETE CASCADE,
    name       TEXT NOT NULL,
    -- text | number | date | select | bool | user | url
    kind       TEXT NOT NULL CHECK (kind IN ('text', 'number', 'date', 'select', 'bool', 'user', 'url')),
    options    TEXT[] NOT NULL DEFAULT '{}',
    required   BOOLEAN NOT NULL DEFAULT FALSE,
    show_in_list BOOLEAN NOT NULL DEFAULT FALSE,
    sort       INT NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX task_field_defs_project_idx ON task_field_defs (project_id ASC);

CREATE TABLE task_field_values (
    task_id  UUID NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
    field_id UUID NOT NULL REFERENCES task_field_defs (id) ON DELETE CASCADE,
    value    JSONB NOT NULL,
    PRIMARY KEY (task_id, field_id)
);

-- Deleted tasks: everything about the task, kept for 30 days so it can be put back.
CREATE TABLE task_trash (
    id         UUID PRIMARY KEY,
    number     BIGINT NOT NULL,
    title      TEXT NOT NULL,
    project_id UUID,
    created_by UUID,
    responsible_id UUID,
    -- The task's row, its subtasks' ids, and the rows of its parts (members, checklist,
    -- comments, files, history, time, links), table by table.
    snapshot   JSONB NOT NULL,
    deleted_by UUID REFERENCES users (id) ON DELETE SET NULL,
    deleted_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX task_trash_deleted_idx ON task_trash (deleted_at ASC);
