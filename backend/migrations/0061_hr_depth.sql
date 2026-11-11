-- HR depth: work reports to one's manager (daily, weekly or monthly), "My day" (today's plan),
-- shift schedules (shifts and who works which, compared with when they actually started),
-- and deputy department heads (who manage and approve along with the head).

ALTER TABLE departments ADD COLUMN deputy_user_id UUID REFERENCES users (id) ON DELETE SET NULL;

-- How often people report: the company's rule, and per person.
ALTER TABLE hr_settings ADD COLUMN report_period TEXT NOT NULL DEFAULT 'none' CHECK (report_period IN ('none', 'daily', 'weekly', 'monthly'));
CREATE TABLE hr_report_rules (
    user_id     UUID PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    period      TEXT CHECK (period IS NULL OR period IN ('none', 'daily', 'weekly', 'monthly')),
    -- Who reads them (default: the nearest department heads).
    reviewer_id UUID REFERENCES users (id) ON DELETE SET NULL
);

CREATE TABLE hr_reports (
    id             UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id        UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    period         TEXT NOT NULL CHECK (period IN ('daily', 'weekly', 'monthly')),
    period_start   DATE NOT NULL,
    period_end     DATE NOT NULL,
    -- What was done, and what comes next.
    body           TEXT,
    plans          TEXT,
    -- Figures gathered when it was sent (tasks done, hours worked…).
    figures        JSONB NOT NULL DEFAULT '{}',
    status         TEXT NOT NULL DEFAULT 'draft' CHECK (status IN ('draft', 'sent', 'reviewed')),
    reviewer_ids   UUID[] NOT NULL DEFAULT '{}',
    sent_at        TIMESTAMPTZ,
    -- good | bad | none (read, no mark)
    mark           TEXT CHECK (mark IS NULL OR mark IN ('good', 'bad', 'none')),
    review_comment TEXT,
    reviewed_by    UUID REFERENCES users (id) ON DELETE SET NULL,
    reviewed_at    TIMESTAMPTZ,
    reminded_at    TIMESTAMPTZ,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (user_id, period, period_start)
);
CREATE INDEX hr_reports_review_idx ON hr_reports USING GIN (reviewer_ids);

-- My day: what someone plans to do today (free text or a task).
CREATE TABLE hr_day_plan (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id    UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    day        DATE NOT NULL,
    title      TEXT NOT NULL,
    task_id    UUID REFERENCES tasks (id) ON DELETE CASCADE,
    done       BOOLEAN NOT NULL DEFAULT FALSE,
    position   INT NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX hr_day_plan_user_idx ON hr_day_plan (user_id ASC, day DESC);

-- Shifts and the rota.
CREATE TABLE hr_shifts (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name          TEXT NOT NULL,
    start_min     INT NOT NULL CHECK (start_min BETWEEN 0 AND 1439),
    -- May end after midnight (a night shift).
    end_min       INT NOT NULL CHECK (end_min BETWEEN 1 AND 2880),
    break_minutes INT NOT NULL DEFAULT 0 CHECK (break_minutes BETWEEN 0 AND 480),
    color         TEXT NOT NULL DEFAULT '#1a73e8',
    is_active     BOOLEAN NOT NULL DEFAULT TRUE,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (end_min > start_min)
);
CREATE TABLE hr_rota (
    user_id  UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    day      DATE NOT NULL,
    shift_id UUID NOT NULL REFERENCES hr_shifts (id) ON DELETE CASCADE,
    PRIMARY KEY (user_id, day)
);
CREATE INDEX hr_rota_day_idx ON hr_rota (day ASC);
