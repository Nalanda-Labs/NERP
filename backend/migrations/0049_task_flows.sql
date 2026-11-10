-- Flows: a queue of similar tasks a team works through. Tasks come in from anyone, are handed
-- out (by the flow's administrator, in turn to the team, or taken by the team themselves), and
-- the flow measures how long tasks wait, how long they take and how many are done in time.

CREATE TABLE task_flows (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    project_id    UUID NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    name          TEXT NOT NULL,
    description   TEXT,
    -- Hands tasks out (manual), and answers for the flow.
    owner_id      UUID NOT NULL REFERENCES users (id),
    -- manual: the administrator assigns; queue: in turn to the team; self: the team takes them.
    distribution  TEXT NOT NULL DEFAULT 'queue' CHECK (distribution IN ('manual', 'queue', 'self')),
    team          UUID[] NOT NULL DEFAULT '{}',
    -- How long a task should take from coming in to being done.
    planned_secs  INT CHECK (planned_secs IS NULL OR planned_secs > 0),
    is_active     BOOLEAN NOT NULL DEFAULT TRUE,
    created_by    UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX task_flows_project_idx ON task_flows (project_id ASC);

ALTER TABLE tasks ADD COLUMN flow_id UUID REFERENCES task_flows (id) ON DELETE SET NULL;
-- In a manual or self-service flow: not handed to anyone yet.
ALTER TABLE tasks ADD COLUMN flow_waiting BOOLEAN NOT NULL DEFAULT FALSE;
-- When work first started on it (for how long tasks wait).
ALTER TABLE tasks ADD COLUMN started_at TIMESTAMPTZ;
CREATE INDEX tasks_flow_idx ON tasks (flow_id ASC, status ASC) WHERE flow_id IS NOT NULL;
