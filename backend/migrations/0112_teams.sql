-- Cross-functional teams: people from any department working together, with a lead, beside
-- the line structure of departments. A person can be in several teams; each team has a chat.
CREATE TABLE teams (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name         TEXT NOT NULL,
    description  TEXT,
    color        TEXT,
    lead_id      UUID REFERENCES users (id) ON DELETE SET NULL,
    created_by   UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE team_members (
    team_id  UUID NOT NULL REFERENCES teams (id) ON DELETE CASCADE,
    user_id  UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    -- What they do in the team ("Design", "QA"…), optional.
    role     TEXT,
    added_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (team_id, user_id)
);
CREATE INDEX team_members_user_idx ON team_members (user_id);

ALTER TABLE im_chats ADD COLUMN team_id UUID UNIQUE REFERENCES teams (id) ON DELETE CASCADE;
