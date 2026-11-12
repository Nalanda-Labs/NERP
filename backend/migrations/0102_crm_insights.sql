-- Deal health and silent clients (src/crm/insights.rs): whether a task is made for a client
-- who stopped ordering, and the alerts made (one per silence: per client and last order).
CREATE TABLE crm_insight_settings (
    id            INT PRIMARY KEY DEFAULT 1 CHECK (id = 1),
    silent_tasks  BOOLEAN NOT NULL DEFAULT FALSE
);
INSERT INTO crm_insight_settings (id) VALUES (1);

CREATE TABLE crm_silent_alerts (
    client_id     UUID NOT NULL REFERENCES crm_items (id) ON DELETE CASCADE,
    last_order_at TIMESTAMPTZ NOT NULL,
    task_id       UUID REFERENCES tasks (id) ON DELETE SET NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (client_id, last_order_at)
);
