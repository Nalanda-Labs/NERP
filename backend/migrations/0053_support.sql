-- Help desk: requests (tickets) from employees and from customers (a public page and an
-- email inbox), worked by the support team under service-level agreements.

-- One row: how the help desk works.
CREATE TABLE support_settings (
    id               BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (id),
    -- The CRM mailbox whose incoming mail becomes tickets, and that replies go out through.
    mailbox_id       UUID REFERENCES crm_mailboxes (id) ON DELETE SET NULL,
    -- The public page where customers send requests (/help).
    portal_enabled   BOOLEAN NOT NULL DEFAULT TRUE,
    portal_intro     TEXT,
    -- Close a ticket that has waited this many days for the customer (0: never).
    auto_close_days  INT NOT NULL DEFAULT 7 CHECK (auto_close_days BETWEEN 0 AND 365),
    -- May a customer reopen a closed ticket by writing to it?
    allow_reopen     BOOLEAN NOT NULL DEFAULT TRUE,
    -- The time zone SLA working hours are in.
    timezone         TEXT NOT NULL DEFAULT 'UTC',
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);
INSERT INTO support_settings (id) VALUES (TRUE);

-- Lists people choose from: categories, criticality, statuses, satisfaction marks and
-- difficulty.
CREATE TABLE support_dictionary (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    kind            TEXT NOT NULL CHECK (kind IN ('category', 'criticality', 'status', 'mark', 'difficulty')),
    name            TEXT NOT NULL,
    description     TEXT,
    sort            INT NOT NULL DEFAULT 100,
    -- New tickets get the default value of each kind.
    is_default      BOOLEAN NOT NULL DEFAULT FALSE,
    -- Categories: who new tickets in this category go to.
    responsible_id  UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX support_dictionary_kind_idx ON support_dictionary (kind ASC, sort ASC);

INSERT INTO support_dictionary (kind, name, sort, is_default) VALUES
    ('category', 'Question', 100, TRUE),
    ('category', 'Problem', 200, FALSE),
    ('category', 'Billing', 300, FALSE),
    ('criticality', 'Low', 100, FALSE),
    ('criticality', 'Middle', 200, TRUE),
    ('criticality', 'High', 300, FALSE),
    ('status', 'Request accepted', 100, TRUE),
    ('status', 'Problem solving in progress', 200, FALSE),
    ('status', 'Successfully solved', 300, FALSE),
    ('status', 'Could not be solved', 400, FALSE),
    ('mark', 'Answer suits the needs', 100, FALSE),
    ('mark', 'Answer is not complete', 200, FALSE),
    ('mark', 'Answer does not suit', 300, FALSE),
    ('difficulty', 'Easy', 100, FALSE),
    ('difficulty', 'Medium', 200, FALSE),
    ('difficulty', 'Hard', 300, FALSE);

-- Service-level agreements: how soon support must answer. A ticket gets the matching SLA
-- with the highest priority; the default one when none matches.
CREATE TABLE support_slas (
    id                UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name              TEXT NOT NULL,
    description       TEXT,
    priority          INT NOT NULL DEFAULT 0,
    is_default        BOOLEAN NOT NULL DEFAULT FALSE,
    -- Working minutes support has to reply after the customer writes.
    response_minutes  INT NOT NULL CHECK (response_minutes > 0),
    -- Warn the responsible person this many working minutes before (0: no warning).
    notice_minutes    INT NOT NULL DEFAULT 0 CHECK (notice_minutes >= 0),
    -- Told when a ticket under this SLA goes overdue, and gets tickets nobody else does.
    responsible_id    UUID REFERENCES users (id) ON DELETE SET NULL,
    -- Which tickets it covers (empty: any).
    category_ids      UUID[] NOT NULL DEFAULT '{}',
    criticality_ids   UUID[] NOT NULL DEFAULT '{}',
    -- any | staff (employees) | external (customers)
    requesters        TEXT NOT NULL DEFAULT 'any' CHECK (requesters IN ('any', 'staff', 'external')),
    -- Working hours per ISO weekday ("1" = Monday): lists of [from, till] minutes of the day.
    -- A weekday that is missing or empty is not worked.
    hours             JSONB NOT NULL DEFAULT '{"1":[[0,1440]],"2":[[0,1440]],"3":[[0,1440]],"4":[[0,1440]],"5":[[0,1440]],"6":[[0,1440]],"7":[[0,1440]]}',
    -- Company holidays (HR) are not worked.
    skip_holidays     BOOLEAN NOT NULL DEFAULT TRUE,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now()
);
INSERT INTO support_slas (name, description, is_default, response_minutes, notice_minutes, skip_holidays)
VALUES ('Standard', 'Every ticket not covered by another agreement: a reply within a day.', TRUE, 1440, 120, FALSE);

CREATE SEQUENCE support_ticket_number;

CREATE TABLE support_tickets (
    id                 UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    number             BIGINT NOT NULL UNIQUE DEFAULT nextval('support_ticket_number'),
    title              TEXT NOT NULL,
    -- staff (an employee, in the app) | portal (the public page) | email
    channel            TEXT NOT NULL CHECK (channel IN ('staff', 'portal', 'email')),
    -- Who asked: an employee, or someone outside known by email.
    requester_id       UUID REFERENCES users (id) ON DELETE SET NULL,
    requester_email    TEXT,
    requester_name     TEXT,
    -- The CRM contact with the requester's email, if there is one.
    contact_id         UUID REFERENCES crm_items (id) ON DELETE SET NULL,
    category_id        UUID REFERENCES support_dictionary (id) ON DELETE SET NULL,
    criticality_id     UUID REFERENCES support_dictionary (id) ON DELETE SET NULL,
    status_id          UUID REFERENCES support_dictionary (id) ON DELETE SET NULL,
    difficulty_id      UUID REFERENCES support_dictionary (id) ON DELETE SET NULL,
    mark_id            UUID REFERENCES support_dictionary (id) ON DELETE SET NULL,
    sla_id             UUID REFERENCES support_slas (id) ON DELETE SET NULL,
    responsible_id     UUID REFERENCES users (id) ON DELETE SET NULL,
    -- Customers open their ticket with a secret link: its hash to find the ticket, and the
    -- link itself encrypted with the data key (for the emails that carry it).
    access_hash        TEXT UNIQUE,
    access_secret      BYTEA,
    -- support (the customer wrote last) | customer (support answered)
    waiting_for        TEXT NOT NULL DEFAULT 'support' CHECK (waiting_for IN ('support', 'customer')),
    -- When the customer's unanswered message came: the SLA clock runs from here.
    waiting_since      TIMESTAMPTZ,
    -- When support's reply is due; NULL when nothing is due (answered, on hold, closed).
    deadline_at        TIMESTAMPTZ,
    notice_at          TIMESTAMPTZ,
    notice_sent        BOOLEAN NOT NULL DEFAULT FALSE,
    is_overdue         BOOLEAN NOT NULL DEFAULT FALSE,
    overdue_count      INT NOT NULL DEFAULT 0,
    -- On hold: no deadline and no automatic closing.
    on_hold            BOOLEAN NOT NULL DEFAULT FALSE,
    messages           INT NOT NULL DEFAULT 0,
    last_message_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    first_response_at  TIMESTAMPTZ,
    minutes_spent      INT NOT NULL DEFAULT 0,
    is_spam            BOOLEAN NOT NULL DEFAULT FALSE,
    closed_at          TIMESTAMPTZ,
    closed_by          UUID REFERENCES users (id) ON DELETE SET NULL,
    auto_closed        BOOLEAN NOT NULL DEFAULT FALSE,
    created_by         UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at         TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX support_tickets_responsible_idx ON support_tickets (responsible_id, closed_at);
CREATE INDEX support_tickets_requester_idx ON support_tickets (requester_id);
CREATE INDEX support_tickets_email_idx ON support_tickets (requester_email);
CREATE INDEX support_tickets_deadline_idx ON support_tickets (deadline_at) WHERE deadline_at IS NOT NULL;

CREATE TABLE support_messages (
    id                UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    ticket_id         UUID NOT NULL REFERENCES support_tickets (id) ON DELETE CASCADE,
    -- message (seen by the customer) | note (support only) | log (what changed)
    kind              TEXT NOT NULL CHECK (kind IN ('message', 'note', 'log')),
    by_support        BOOLEAN NOT NULL,
    author_id         UUID REFERENCES users (id) ON DELETE SET NULL,
    author_email      TEXT,
    author_name       TEXT,
    body              TEXT NOT NULL,
    file_ids          UUID[] NOT NULL DEFAULT '{}',
    -- Time support spent on this answer.
    minutes_spent     INT NOT NULL DEFAULT 0,
    -- Support answered after the deadline.
    is_overdue        BOOLEAN NOT NULL DEFAULT FALSE,
    -- Email threading: the Message-ID this came in with or went out with.
    email_message_id  TEXT,
    -- message: queued | sent | failed, when it goes out by email.
    email_status      TEXT,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX support_messages_ticket_idx ON support_messages (ticket_id, created_at ASC);
CREATE INDEX support_messages_email_idx ON support_messages (email_message_id) WHERE email_message_id IS NOT NULL;

-- Frequently used answers support can insert into a reply.
CREATE TABLE support_answers (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    title       TEXT NOT NULL,
    body        TEXT NOT NULL,
    sort        INT NOT NULL DEFAULT 100,
    created_by  UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
