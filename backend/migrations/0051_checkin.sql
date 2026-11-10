-- Check-in: once a day each person says whether they are working today and where (office,
-- home, remotely, fieldwork or their own words), optionally posting it to a chat with a
-- message and their location; or that they aren't working, and why. Department views show who
-- is working where, by day and by month.

-- One row of company settings (HR managers change them).
CREATE TABLE checkin_settings (
    id            BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (id),
    enabled       BOOLEAN NOT NULL DEFAULT TRUE,
    -- People may attach where they are (their browser's location) to a check-in.
    geo_enabled   BOOLEAN NOT NULL DEFAULT TRUE,
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);
INSERT INTO checkin_settings (id) VALUES (TRUE);

CREATE TABLE checkins (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id         UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    -- The person's own date (in their time zone).
    day             DATE NOT NULL,
    -- working | not_working | cancelled (checked in as working, then said they aren't)
    status          TEXT NOT NULL CHECK (status IN ('working', 'not_working', 'cancelled')),
    -- office | home | remote | field | custom (working only)
    location        TEXT CHECK (location IN ('office', 'home', 'remote', 'field', 'custom')),
    location_text   TEXT,
    message         TEXT,
    -- The chat it was posted to.
    chat_id         UUID REFERENCES im_chats (id) ON DELETE SET NULL,
    -- Where they were, when they shared it.
    latitude        DOUBLE PRECISION,
    longitude       DOUBLE PRECISION,
    address         TEXT,
    -- Not working: illness | sick_leave | time_off | vacation | custom, and the words shown.
    reason          TEXT CHECK (reason IN ('illness', 'sick_leave', 'time_off', 'vacation', 'custom')),
    reason_text     TEXT,
    cancelled_at    TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (user_id, day),
    CHECK ((status = 'working') = (location IS NOT NULL) OR status = 'cancelled'),
    CHECK (status = 'working' OR reason IS NOT NULL),
    CHECK ((latitude IS NULL) = (longitude IS NULL))
);
CREATE INDEX checkins_day_idx ON checkins (day ASC);

-- Each person's choices, remembered for next time, and the reminder.
CREATE TABLE checkin_prefs (
    user_id          UUID PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    location         TEXT NOT NULL DEFAULT 'office' CHECK (location IN ('office', 'home', 'remote', 'field', 'custom')),
    location_text    TEXT,
    message          TEXT,
    send_message     BOOLEAN NOT NULL DEFAULT TRUE,
    send_geo         BOOLEAN NOT NULL DEFAULT FALSE,
    chat_id          UUID REFERENCES im_chats (id) ON DELETE SET NULL,
    -- Start the work day (HR → working time) on checking in as working.
    start_workday    BOOLEAN NOT NULL DEFAULT FALSE,
    -- The department last looked at.
    department_id    UUID REFERENCES departments (id) ON DELETE SET NULL,
    -- The reminder: shown on working days until checked in (to people who have checked in before,
    -- or are in a chat check-ins were posted to). Muted for good (forever) or until a time.
    muted_forever    BOOLEAN NOT NULL DEFAULT FALSE,
    muted_until      TIMESTAMPTZ,
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);
