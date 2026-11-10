-- Platform services: notifications, background jobs, invitations and two-factor auth.

-- ---------- accounts ----------
-- An invited user exists (so they can be put in departments, assigned roles, …) but cannot
-- sign in until they accept the invitation and choose a password.
ALTER TABLE users ADD COLUMN invite_pending BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE users ADD COLUMN invited_at TIMESTAMPTZ;
-- Base32 TOTP secret; NULL when two-factor authentication is off.
ALTER TABLE users ADD COLUMN totp_secret TEXT;
ALTER TABLE users ADD COLUMN totp_enabled_at TIMESTAMPTZ;

CREATE TABLE user_recovery_codes (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id     UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    code_hash   TEXT NOT NULL,
    used_at     TIMESTAMPTZ,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX user_recovery_codes_user_idx ON user_recovery_codes (user_id);

-- ---------- notifications ----------
CREATE TABLE notifications (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id     UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    -- Machine-readable kind, e.g. "department.added"; modules add their own.
    kind        TEXT NOT NULL,
    title       TEXT NOT NULL,
    body        TEXT,
    -- In-app path to open when the notification is clicked.
    link        TEXT,
    actor_id    UUID REFERENCES users (id) ON DELETE SET NULL,
    read_at     TIMESTAMPTZ,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX notifications_user_idx ON notifications (user_id, created_at DESC);

-- ---------- background jobs ----------
CREATE TABLE jobs (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    kind          TEXT NOT NULL,
    payload       JSONB NOT NULL DEFAULT '{}'::jsonb,
    -- pending → running → done | failed (after max_attempts) ; running jobs whose lock
    -- expires are put back to pending, so a crashed worker never loses work.
    status        TEXT NOT NULL DEFAULT 'pending'
                  CHECK (status IN ('pending', 'running', 'done', 'failed')),
    run_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    attempts      INT NOT NULL DEFAULT 0,
    max_attempts  INT NOT NULL DEFAULT 5,
    locked_by     TEXT,
    locked_until  TIMESTAMPTZ,
    last_error    TEXT,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX jobs_due_idx ON jobs (status, run_at);
CREATE INDEX jobs_created_idx ON jobs (created_at DESC);
