-- Meetings: planned with an agenda, held with decisions and minutes, followed up by the next
-- meeting in the series, which carries the unfinished topics.

CREATE TABLE meetings (
    id               UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    title            TEXT NOT NULL,
    description      TEXT,
    place            TEXT,
    starts_at        TIMESTAMPTZ NOT NULL,
    duration_minutes INT NOT NULL DEFAULT 60 CHECK (duration_minutes BETWEEN 5 AND 1440),
    timezone         TEXT NOT NULL DEFAULT 'UTC',
    -- prepare (agenda being written) | in_progress | closed (minutes final)
    state            TEXT NOT NULL DEFAULT 'prepare' CHECK (state IN ('prepare', 'in_progress', 'closed')),
    -- A workgroup's meeting is visible to the workgroup's members.
    project_id       UUID REFERENCES projects (id) ON DELETE SET NULL,
    -- The meeting this one follows up (a series).
    parent_id        UUID REFERENCES meetings (id) ON DELETE SET NULL,
    -- Free text of the minutes, besides the decisions recorded per topic.
    minutes          TEXT,
    -- The event on the participants' calendars.
    event_id         UUID REFERENCES cal_events (id) ON DELETE SET NULL,
    file_ids         UUID[] NOT NULL DEFAULT '{}',
    started_at       TIMESTAMPTZ,
    closed_at        TIMESTAMPTZ,
    created_by       UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX meetings_starts_idx ON meetings (starts_at DESC);
CREATE INDEX meetings_parent_idx ON meetings (parent_id);

CREATE TABLE meeting_members (
    meeting_id  UUID NOT NULL REFERENCES meetings (id) ON DELETE CASCADE,
    user_id     UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    -- owner (runs it) | keeper (writes the minutes) | member
    role        TEXT NOT NULL DEFAULT 'member' CHECK (role IN ('owner', 'keeper', 'member')),
    -- Marked during the meeting, for the minutes.
    attended    BOOLEAN,
    PRIMARY KEY (meeting_id, user_id)
);
CREATE INDEX meeting_members_user_idx ON meeting_members (user_id);

-- A topic lives on across the meetings of a series (its history: what was reported and
-- decided each time).
CREATE TABLE meeting_topics (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    title       TEXT NOT NULL,
    created_by  UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- A topic on one meeting's agenda.
CREATE TABLE meeting_agenda (
    id               UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    meeting_id       UUID NOT NULL REFERENCES meetings (id) ON DELETE CASCADE,
    topic_id         UUID NOT NULL REFERENCES meeting_topics (id) ON DELETE CASCADE,
    -- A sub-point of another agenda entry.
    parent_id        UUID REFERENCES meeting_agenda (id) ON DELETE CASCADE,
    sort             INT NOT NULL DEFAULT 0,
    description      TEXT,
    -- Who presents it and reports on it.
    responsible_ids  UUID[] NOT NULL DEFAULT '{}',
    duration_minutes INT CHECK (duration_minutes IS NULL OR duration_minutes BETWEEN 1 AND 600),
    -- What was decided, recorded during the meeting.
    decision         TEXT,
    -- open | done (settled) | carried (moved to the next meeting)
    status           TEXT NOT NULL DEFAULT 'open' CHECK (status IN ('open', 'done', 'carried')),
    -- A task made from this topic.
    task_id          UUID REFERENCES tasks (id) ON DELETE SET NULL,
    proposed_by      UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX meeting_agenda_meeting_idx ON meeting_agenda (meeting_id, sort);
CREATE INDEX meeting_agenda_topic_idx ON meeting_agenda (topic_id);

-- What a responsible person reports on a topic before or at the meeting.
CREATE TABLE meeting_reports (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    agenda_id   UUID NOT NULL REFERENCES meeting_agenda (id) ON DELETE CASCADE,
    user_id     UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    body        TEXT NOT NULL,
    file_ids    UUID[] NOT NULL DEFAULT '{}',
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (agenda_id, user_id)
);
