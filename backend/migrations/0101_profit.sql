-- True profit (src/crm/profit.rs): what an hour of each person's time costs the company (for
-- support and task time spent on clients), a default for everyone else, and the yearly rate
-- money owed costs while invoices are paid late.
CREATE TABLE user_costs (
    user_id    UUID PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    hourly     NUMERIC(14, 2) NOT NULL CHECK (hourly >= 0),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE profit_settings (
    id                 INT PRIMARY KEY DEFAULT 1 CHECK (id = 1),
    -- In the base currency.
    default_hourly     NUMERIC(14, 2) NOT NULL DEFAULT 0 CHECK (default_hourly >= 0),
    -- Percent per year (e.g. 8): money tied up in late invoices.
    financing_rate     NUMERIC(6, 2) NOT NULL DEFAULT 8 CHECK (financing_rate BETWEEN 0 AND 100)
);
INSERT INTO profit_settings (id) VALUES (1);
