-- Retention rules: how long each kind of data is kept, enforced daily and logged.
CREATE TABLE retention_rules (
    key         TEXT PRIMARY KEY,
    enabled     BOOLEAN NOT NULL DEFAULT FALSE,
    keep_days   INT NOT NULL CHECK (keep_days >= 1),
    action      TEXT NOT NULL CHECK (action IN ('delete', 'anonymise')),
    updated_by  UUID REFERENCES users (id) ON DELETE SET NULL,
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE retention_runs (
    id         BIGSERIAL PRIMARY KEY,
    key        TEXT NOT NULL,
    action     TEXT NOT NULL,
    keep_days  INT NOT NULL,
    affected   BIGINT NOT NULL DEFAULT 0,
    error      TEXT,
    run_by     UUID REFERENCES users (id) ON DELETE SET NULL,
    ran_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX retention_runs_idx ON retention_runs (ran_at DESC);
