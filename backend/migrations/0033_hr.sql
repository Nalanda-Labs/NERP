-- HR: the work day (time tracking) and absences (time off with approval).

-- One row: how the company works.
CREATE TABLE hr_settings (
    id                     BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (id),
    -- ISO weekdays that are working days (1 = Monday).
    work_weekdays          INT[] NOT NULL DEFAULT '{1,2,3,4,5}',
    workday_hours          NUMERIC(4, 2) NOT NULL DEFAULT 8,
    annual_vacation_days   NUMERIC(5, 1) NOT NULL DEFAULT 20,
    updated_at             TIMESTAMPTZ NOT NULL DEFAULT now()
);
INSERT INTO hr_settings (id) VALUES (TRUE);

-- Company holidays: not working days, not counted as vacation.
CREATE TABLE hr_holidays (
    day  DATE PRIMARY KEY,
    name TEXT NOT NULL
);

-- A person's vacation allowance for a year, when it differs from the company's.
CREATE TABLE hr_allowances (
    user_id UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    year    INT NOT NULL,
    days    NUMERIC(5, 1) NOT NULL CHECK (days >= 0),
    PRIMARY KEY (user_id, year)
);

CREATE TABLE hr_absences (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id       UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    -- vacation | sick | personal | business_trip | other
    kind          TEXT NOT NULL CHECK (kind IN ('vacation', 'sick', 'personal', 'business_trip', 'other')),
    starts_on     DATE NOT NULL,
    ends_on       DATE NOT NULL,
    note          TEXT,
    -- pending | approved | rejected | cancelled
    status        TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'approved', 'rejected', 'cancelled')),
    decided_by    UUID REFERENCES users (id) ON DELETE SET NULL,
    decided_at    TIMESTAMPTZ,
    decision_note TEXT,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (ends_on >= starts_on)
);
CREATE INDEX hr_absences_dates_idx ON hr_absences (starts_on ASC, ends_on ASC);
CREATE INDEX hr_absences_user_idx ON hr_absences (user_id ASC, starts_on DESC);

-- The work day: started, paused (breaks), ended.
CREATE TABLE hr_work_days (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id     UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    -- The person's local date when they started.
    day         DATE NOT NULL,
    started_at  TIMESTAMPTZ NOT NULL,
    ended_at    TIMESTAMPTZ,
    note        TEXT,
    -- A correction by a manager, and why.
    edited_by   UUID REFERENCES users (id) ON DELETE SET NULL,
    edit_reason TEXT,
    UNIQUE (user_id, day)
);
CREATE INDEX hr_work_days_user_idx ON hr_work_days (user_id ASC, day DESC);

CREATE TABLE hr_work_breaks (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    day_id     UUID NOT NULL REFERENCES hr_work_days (id) ON DELETE CASCADE,
    started_at TIMESTAMPTZ NOT NULL,
    ended_at   TIMESTAMPTZ
);
CREATE INDEX hr_work_breaks_day_idx ON hr_work_breaks (day_id ASC);
