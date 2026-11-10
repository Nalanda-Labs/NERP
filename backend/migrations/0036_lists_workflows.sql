-- Lists (the company's own small databases) and business processes (workflows) on them.

CREATE TABLE lists (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name        TEXT NOT NULL,
    description TEXT,
    -- Everyone may add and see items, or only the people and departments in list_access.
    to_all      BOOLEAN NOT NULL DEFAULT TRUE,
    created_by  UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_number BIGINT NOT NULL DEFAULT 0
);

-- read: see items; add: also add items (and change their own); manage: everything, settings.
CREATE TABLE list_access (
    list_id       UUID NOT NULL REFERENCES lists (id) ON DELETE CASCADE,
    user_id       UUID REFERENCES users (id) ON DELETE CASCADE,
    department_id UUID REFERENCES departments (id) ON DELETE CASCADE,
    level         TEXT NOT NULL CHECK (level IN ('read', 'add', 'manage')),
    CHECK ((user_id IS NULL) <> (department_id IS NULL))
);
CREATE INDEX list_access_list_idx ON list_access (list_id ASC);

CREATE TABLE list_fields (
    id       UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    list_id  UUID NOT NULL REFERENCES lists (id) ON DELETE CASCADE,
    code     TEXT NOT NULL,
    label    TEXT NOT NULL,
    -- text | textarea | number | money | date | select | user | checkbox | file | crm
    kind     TEXT NOT NULL CHECK (kind IN ('text', 'textarea', 'number', 'money', 'date', 'select', 'user', 'checkbox', 'file', 'crm')),
    options  TEXT[] NOT NULL DEFAULT '{}',
    required BOOLEAN NOT NULL DEFAULT FALSE,
    sort     INT NOT NULL DEFAULT 0,
    UNIQUE (list_id, code)
);

CREATE TABLE list_items (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    list_id    UUID NOT NULL REFERENCES lists (id) ON DELETE CASCADE,
    number     BIGINT NOT NULL,
    title      TEXT NOT NULL,
    "values"   JSONB NOT NULL DEFAULT '{}',
    created_by UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_by UUID REFERENCES users (id) ON DELETE SET NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (list_id, number)
);
CREATE INDEX list_items_list_idx ON list_items (list_id ASC, created_at DESC);

-- A business process: steps (a tree, see workflows.rs) started on create, on change, or by hand.
CREATE TABLE wf_templates (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    list_id    UUID NOT NULL REFERENCES lists (id) ON DELETE CASCADE,
    name       TEXT NOT NULL,
    -- create | update | manual
    trigger    TEXT NOT NULL CHECK (trigger IN ('create', 'update', 'manual')),
    steps      JSONB NOT NULL DEFAULT '[]',
    is_active  BOOLEAN NOT NULL DEFAULT TRUE,
    created_by UUID REFERENCES users (id) ON DELETE SET NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE wf_instances (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    template_id UUID REFERENCES wf_templates (id) ON DELETE SET NULL,
    name        TEXT NOT NULL,
    item_id     UUID NOT NULL REFERENCES list_items (id) ON DELETE CASCADE,
    -- running | waiting | completed | rejected | cancelled | failed
    status      TEXT NOT NULL DEFAULT 'running',
    -- The steps as they were when it started, and where it is in them.
    steps       JSONB NOT NULL,
    cursor      JSONB NOT NULL DEFAULT '[]',
    started_by  UUID REFERENCES users (id) ON DELETE SET NULL,
    started_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at TIMESTAMPTZ,
    error       TEXT
);
CREATE INDEX wf_instances_item_idx ON wf_instances (item_id ASC, started_at DESC);

-- Approvals waiting for people.
CREATE TABLE wf_approvals (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    instance_id UUID NOT NULL REFERENCES wf_instances (id) ON DELETE CASCADE,
    title       TEXT NOT NULL,
    approvers   UUID[] NOT NULL,
    -- any: one decision settles it; all: everyone must approve (one rejection settles it).
    mode        TEXT NOT NULL CHECK (mode IN ('any', 'all')),
    -- pending | approved | rejected | cancelled
    status      TEXT NOT NULL DEFAULT 'pending',
    -- [{"user_id": …, "decision": "approved", "comment": …, "at": …}]
    decisions   JSONB NOT NULL DEFAULT '[]',
    -- Where the process goes on: the approval step's place in the tree.
    at          JSONB NOT NULL,
    due_at      TIMESTAMPTZ,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    decided_at  TIMESTAMPTZ
);
CREATE INDEX wf_approvals_pending_idx ON wf_approvals (status ASC) WHERE status = 'pending';

CREATE TABLE wf_log (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    instance_id UUID NOT NULL REFERENCES wf_instances (id) ON DELETE CASCADE,
    user_id     UUID REFERENCES users (id) ON DELETE SET NULL,
    text        TEXT NOT NULL,
    at          TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
);
CREATE INDEX wf_log_instance_idx ON wf_log (instance_id ASC, at ASC);
