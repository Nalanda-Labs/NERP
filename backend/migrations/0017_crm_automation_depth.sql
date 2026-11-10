-- Deeper CRM automation: robots timed from a date field, robots that run one after
-- another or wait for something, scheduled (recurring) automation, stage time limits, and
-- what the automation report needs.

-- When a robot runs: 'stage' = on entering its stage (plus delay_minutes); 'field' = at a
-- date field of the record plus offset_minutes (negative = before).
ALTER TABLE crm_robots ADD COLUMN timing TEXT NOT NULL DEFAULT 'stage' CHECK (timing IN ('stage', 'field'));
ALTER TABLE crm_robots ADD COLUMN timing_field TEXT;
ALTER TABLE crm_robots ADD COLUMN offset_minutes INT NOT NULL DEFAULT 0 CHECK (offset_minutes BETWEEN -525600 AND 525600);
-- Start only when the robot before it (in the same stage) has finished.
ALTER TABLE crm_robots ADD COLUMN after_previous BOOLEAN NOT NULL DEFAULT FALSE;
-- { "until": "conditions" | "email_received" | "activity_done", "conditions": [...],
--   "timeout_minutes": 1440, "on_timeout": "run" | "skip" }
ALTER TABLE crm_robots ADD COLUMN wait JSONB;

-- Recurring automation: on a schedule, run an action for every record of a pipeline that
-- is in the chosen stages (all open stages when none) and matches the conditions.
CREATE TABLE crm_schedules (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    entity_type   SMALLINT NOT NULL REFERENCES crm_entity_types (id) ON DELETE CASCADE,
    category_id   UUID NOT NULL REFERENCES crm_categories (id) ON DELETE CASCADE,
    name          TEXT NOT NULL,
    -- daily | weekly | monthly
    frequency     TEXT NOT NULL CHECK (frequency IN ('daily', 'weekly', 'monthly')),
    -- ISO weekdays (1 = Monday) for weekly schedules.
    weekdays      SMALLINT[] NOT NULL DEFAULT '{}',
    -- Day of the month for monthly schedules; past the month's end means its last day.
    month_day     SMALLINT NOT NULL DEFAULT 1 CHECK (month_day BETWEEN 1 AND 31),
    at_time       TIME NOT NULL DEFAULT '09:00',
    timezone      TEXT NOT NULL DEFAULT 'UTC',
    stage_ids     UUID[] NOT NULL DEFAULT '{}',
    conditions    JSONB NOT NULL DEFAULT '[]',
    action        TEXT NOT NULL,
    params        JSONB NOT NULL DEFAULT '{}',
    is_active     BOOLEAN NOT NULL DEFAULT TRUE,
    next_run_at   TIMESTAMPTZ,
    last_run_at   TIMESTAMPTZ,
    last_count    INT,
    created_by    UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX crm_schedules_category_idx ON crm_schedules (category_id ASC);
CREATE INDEX crm_schedules_due_idx ON crm_schedules (next_run_at ASC) WHERE is_active;

-- Runs: statuses are now pending (waits for the previous robot) | scheduled | waiting |
-- running | done | failed | skipped.
ALTER TABLE crm_robot_runs ADD COLUMN schedule_id UUID REFERENCES crm_schedules (id) ON DELETE SET NULL;
ALTER TABLE crm_robot_runs ADD COLUMN category_id UUID;
ALTER TABLE crm_robot_runs ADD COLUMN action TEXT;
ALTER TABLE crm_robot_runs ADD COLUMN after_run UUID;
ALTER TABLE crm_robot_runs ADD COLUMN wait_until TIMESTAMPTZ;
ALTER TABLE crm_robot_runs ADD COLUMN attempts INT NOT NULL DEFAULT 0;
-- Where a paused action resumes (e.g. the document it waits for), and whether a wait is over.
ALTER TABLE crm_robot_runs ADD COLUMN data JSONB NOT NULL DEFAULT '{}';
UPDATE crm_robot_runs r SET category_id = b.category_id, action = b.action FROM crm_robots b WHERE b.id = r.robot_id;
CREATE INDEX crm_robot_runs_open_idx ON crm_robot_runs (item_id ASC) WHERE status IN ('pending', 'scheduled', 'waiting');
CREATE INDEX crm_robot_runs_after_idx ON crm_robot_runs (after_run ASC) WHERE after_run IS NOT NULL;
CREATE INDEX crm_robot_runs_report_idx ON crm_robot_runs (category_id ASC, created_at DESC);

-- Stage time limits: a record that stays longer is flagged, and its responsible person
-- (and, with escalation, their department heads) is notified.
ALTER TABLE crm_stages ADD COLUMN sla_minutes INT CHECK (sla_minutes BETWEEN 1 AND 525600);
ALTER TABLE crm_stages ADD COLUMN sla_escalate BOOLEAN NOT NULL DEFAULT FALSE;
