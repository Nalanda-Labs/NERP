-- Field visits: an activity of kind 'visit' at an address, planned into a day's route, with
-- check-in and check-out where it happened.
ALTER TABLE crm_activities DROP CONSTRAINT crm_activities_kind_check;
ALTER TABLE crm_activities ADD CONSTRAINT crm_activities_kind_check CHECK (kind IN ('todo', 'call', 'meeting', 'email', 'visit'));

CREATE TABLE field_visits (
    activity_id       UUID PRIMARY KEY REFERENCES crm_activities (id) ON DELETE CASCADE,
    address           JSONB,
    lat               DOUBLE PRECISION,
    lng               DOUBLE PRECISION,
    duration_minutes  INT NOT NULL DEFAULT 30 CHECK (duration_minutes BETWEEN 5 AND 720),
    -- A fixed time is kept when the day is planned; otherwise the plan sets it.
    fixed_time        BOOLEAN NOT NULL DEFAULT FALSE,
    route_order       INT,
    checked_in_at     TIMESTAMPTZ,
    in_lat            DOUBLE PRECISION,
    in_lng            DOUBLE PRECISION,
    in_accuracy_m     DOUBLE PRECISION,
    in_distance_m     INT,
    checked_out_at    TIMESTAMPTZ,
    out_lat           DOUBLE PRECISION,
    out_lng           DOUBLE PRECISION,
    notes             TEXT
);

-- Addresses already looked up (by the configured geocoder).
CREATE TABLE geo_cache (
    query       TEXT PRIMARY KEY,
    lat         DOUBLE PRECISION,
    lng         DOUBLE PRECISION,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
