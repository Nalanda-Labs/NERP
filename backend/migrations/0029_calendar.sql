-- Company calendar: events (meetings, appointments, all-day events), who is invited and
-- their answers, repeating events (with occurrences removed one by one), reminders, and a
-- private subscription link per person for other calendar apps.

CREATE TABLE cal_events (
    id             UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    owner_id       UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    title          TEXT NOT NULL,
    description    TEXT,
    location       TEXT,
    -- Timed events: real instants. All-day events: midnight UTC of the first day, and of the
    -- day after the last.
    starts_at      TIMESTAMPTZ NOT NULL,
    ends_at        TIMESTAMPTZ NOT NULL,
    all_day        BOOLEAN NOT NULL DEFAULT FALSE,
    -- The owner's time zone when it was saved: repeats keep their wall-clock time in it.
    timezone       TEXT NOT NULL DEFAULT 'UTC',
    -- Repeat rule (same shape as recurring tasks); NULL = once.
    recurrence     JSONB,
    color          TEXT,
    item_id        UUID REFERENCES crm_items (id) ON DELETE SET NULL,
    remind_minutes INT,
    -- Others see "Busy" instead of the details.
    is_private     BOOLEAN NOT NULL DEFAULT FALSE,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (ends_at >= starts_at)
);
CREATE INDEX cal_events_owner_idx ON cal_events (owner_id ASC, starts_at ASC);
CREATE INDEX cal_events_item_idx ON cal_events (item_id ASC) WHERE item_id IS NOT NULL;

CREATE TABLE cal_attendees (
    event_id UUID NOT NULL REFERENCES cal_events (id) ON DELETE CASCADE,
    user_id  UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    -- pending | accepted | declined | tentative
    status   TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'accepted', 'declined', 'tentative')),
    PRIMARY KEY (event_id, user_id)
);
CREATE INDEX cal_attendees_user_idx ON cal_attendees (user_id ASC);

-- Occurrences of a repeating event that were removed.
CREATE TABLE cal_exceptions (
    event_id         UUID NOT NULL REFERENCES cal_events (id) ON DELETE CASCADE,
    occurrence_start TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (event_id, occurrence_start)
);

CREATE TABLE cal_reminders_sent (
    event_id         UUID NOT NULL REFERENCES cal_events (id) ON DELETE CASCADE,
    user_id          UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    occurrence_start TIMESTAMPTZ NOT NULL,
    sent_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (event_id, user_id, occurrence_start)
);

-- A person's subscription link (only its hash is stored).
CREATE TABLE cal_feeds (
    user_id    UUID PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    token_hash TEXT NOT NULL UNIQUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
