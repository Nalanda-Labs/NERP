-- Scrum: a third kind of group (a Scrum team) with a backlog, sprints, epics, story points,
-- a definition of done, and the history that burndown and velocity charts are drawn from.

ALTER TABLE projects DROP CONSTRAINT projects_kind_check;
ALTER TABLE projects ADD CONSTRAINT projects_kind_check CHECK (kind IN ('group', 'project', 'scrum'));
ALTER TABLE projects ADD COLUMN scrum_master_id UUID REFERENCES users (id) ON DELETE SET NULL;
-- Length of a new sprint, in days.
ALTER TABLE projects ADD COLUMN sprint_days INT NOT NULL DEFAULT 14 CHECK (sprint_days BETWEEN 1 AND 90);

-- Large pieces of work that tasks belong to.
CREATE TABLE scrum_epics (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    project_id  UUID NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    name        TEXT NOT NULL,
    description TEXT,
    color       TEXT NOT NULL DEFAULT 'violet',
    created_by  UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX scrum_epics_project_idx ON scrum_epics (project_id ASC, created_at ASC);

-- planned → active (one at a time per team) → completed.
CREATE TABLE scrum_sprints (
    id               UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    project_id       UUID NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    name             TEXT NOT NULL,
    goal             TEXT,
    status           TEXT NOT NULL DEFAULT 'planned' CHECK (status IN ('planned', 'active', 'completed')),
    sort             DOUBLE PRECISION NOT NULL DEFAULT 0,
    starts_at        TIMESTAMPTZ,
    ends_at          TIMESTAMPTZ,
    -- Points in the sprint when it started, and points finished by its end.
    committed_points DOUBLE PRECISION,
    completed_points DOUBLE PRECISION,
    started_at       TIMESTAMPTZ,
    completed_at     TIMESTAMPTZ,
    created_by       UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX scrum_sprints_project_idx ON scrum_sprints (project_id ASC, sort ASC);
CREATE UNIQUE INDEX scrum_sprints_one_active_uidx ON scrum_sprints (project_id) WHERE status = 'active';

-- What a sprint held when it ended (unfinished tasks move on, the record stays).
CREATE TABLE scrum_sprint_results (
    sprint_id UUID NOT NULL REFERENCES scrum_sprints (id) ON DELETE CASCADE,
    task_id   UUID NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
    points    DOUBLE PRECISION,
    done      BOOLEAN NOT NULL,
    PRIMARY KEY (sprint_id, task_id)
);

-- Changes to an active sprint's scope (tasks added or taken out after it started), for the
-- burndown chart.
CREATE TABLE scrum_scope_changes (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    sprint_id  UUID NOT NULL REFERENCES scrum_sprints (id) ON DELETE CASCADE,
    task_id    UUID REFERENCES tasks (id) ON DELETE SET NULL,
    -- Positive when points came in, negative when they left.
    points     DOUBLE PRECISION NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX scrum_scope_changes_sprint_idx ON scrum_scope_changes (sprint_id ASC, created_at ASC);

-- The team's definition of done: checked before a task is finished.
CREATE TABLE scrum_dod_items (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    project_id UUID NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    title      TEXT NOT NULL,
    sort       INT NOT NULL DEFAULT 0
);
CREATE INDEX scrum_dod_project_idx ON scrum_dod_items (project_id ASC, sort ASC);

ALTER TABLE tasks ADD COLUMN story_points DOUBLE PRECISION CHECK (story_points IS NULL OR story_points >= 0);
ALTER TABLE tasks ADD COLUMN epic_id UUID REFERENCES scrum_epics (id) ON DELETE SET NULL;
-- NULL in a Scrum team's task: the task is in the backlog.
ALTER TABLE tasks ADD COLUMN sprint_id UUID REFERENCES scrum_sprints (id) ON DELETE SET NULL;
-- Order in the backlog or the sprint plan.
ALTER TABLE tasks ADD COLUMN backlog_sort DOUBLE PRECISION NOT NULL DEFAULT 0;
-- When the definition of done was confirmed for the task.
ALTER TABLE tasks ADD COLUMN dod_confirmed_at TIMESTAMPTZ;
CREATE INDEX tasks_sprint_idx ON tasks (sprint_id ASC) WHERE sprint_id IS NOT NULL;
CREATE INDEX tasks_epic_idx ON tasks (epic_id ASC) WHERE epic_id IS NOT NULL;
