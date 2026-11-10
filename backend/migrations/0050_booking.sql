-- Booking: appointments with the company's resources (people, rooms, equipment), with working
-- hours, services and extras, statuses, a waiting list, public self-booking forms, client
-- reminders, and a CRM contact (and deal) for each client.

-- One row of settings.
CREATE TABLE booking_settings (
    id                BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (id),
    -- Email the client a confirmation and, this many hours before, a reminder (0 = none).
    confirm_email     BOOLEAN NOT NULL DEFAULT TRUE,
    remind_hours      INT NOT NULL DEFAULT 24 CHECK (remind_hours BETWEEN 0 AND 168),
    -- Clients from forms become CRM contacts; with a pipeline, each booking also a deal.
    make_contact      BOOLEAN NOT NULL DEFAULT TRUE,
    deal_category_id  UUID REFERENCES crm_categories (id) ON DELETE SET NULL,
    currency          TEXT NOT NULL DEFAULT 'USD',
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now()
);
INSERT INTO booking_settings (id) VALUES (TRUE);

CREATE TABLE booking_resources (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name                TEXT NOT NULL,
    -- person | room | equipment | other
    kind                TEXT NOT NULL DEFAULT 'person' CHECK (kind IN ('person', 'room', 'equipment', 'other')),
    description         TEXT,
    -- A person resource is an employee: their bookings go on their calendar.
    user_id             UUID REFERENCES users (id) ON DELETE SET NULL,
    -- How many bookings may overlap (a room for 10, two massage chairs…).
    capacity            INT NOT NULL DEFAULT 1 CHECK (capacity BETWEEN 1 AND 1000),
    -- Staff may book over capacity (overbooking).
    allow_overbooking   BOOLEAN NOT NULL DEFAULT FALSE,
    color               TEXT NOT NULL DEFAULT 'sky',
    timezone            TEXT NOT NULL DEFAULT 'UTC',
    -- Slots offered to clients start every so many minutes.
    slot_minutes        INT NOT NULL DEFAULT 30 CHECK (slot_minutes BETWEEN 5 AND 1440),
    buffer_minutes      INT NOT NULL DEFAULT 0 CHECK (buffer_minutes BETWEEN 0 AND 480),
    min_notice_minutes  INT NOT NULL DEFAULT 60 CHECK (min_notice_minutes >= 0),
    max_days_ahead      INT NOT NULL DEFAULT 60 CHECK (max_days_ahead BETWEEN 1 AND 730),
    is_active           BOOLEAN NOT NULL DEFAULT TRUE,
    sort                INT NOT NULL DEFAULT 0,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Working hours: intervals per weekday (1 = Monday), in the resource's time zone.
CREATE TABLE booking_hours (
    resource_id UUID NOT NULL REFERENCES booking_resources (id) ON DELETE CASCADE,
    weekday     INT NOT NULL CHECK (weekday BETWEEN 1 AND 7),
    start_min   INT NOT NULL CHECK (start_min BETWEEN 0 AND 1439),
    end_min     INT NOT NULL CHECK (end_min BETWEEN 1 AND 1440),
    CHECK (end_min > start_min),
    PRIMARY KEY (resource_id, weekday, start_min)
);

-- Days a resource is not available (holiday, repair…).
CREATE TABLE booking_days_off (
    resource_id UUID NOT NULL REFERENCES booking_resources (id) ON DELETE CASCADE,
    day         DATE NOT NULL,
    note        TEXT,
    PRIMARY KEY (resource_id, day)
);

CREATE TABLE booking_services (
    id               UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name             TEXT NOT NULL,
    description      TEXT,
    -- Minutes for short bookings; whole days for long ones (a car, a holiday flat).
    duration_minutes INT NOT NULL CHECK (duration_minutes BETWEEN 5 AND 525600),
    price            NUMERIC(14, 2) NOT NULL DEFAULT 0 CHECK (price >= 0),
    -- An extra added to a booking (towels, a projector…), not booked on its own.
    is_extra         BOOLEAN NOT NULL DEFAULT FALSE,
    -- Which resources provide it (empty: any).
    resource_ids     UUID[] NOT NULL DEFAULT '{}',
    is_active        BOOLEAN NOT NULL DEFAULT TRUE,
    sort             INT NOT NULL DEFAULT 0,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE SEQUENCE booking_number_seq;

CREATE TABLE bookings (
    id               UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    number           BIGINT NOT NULL DEFAULT nextval('booking_number_seq') UNIQUE,
    starts_at        TIMESTAMPTZ NOT NULL,
    ends_at          TIMESTAMPTZ NOT NULL,
    -- booked → confirmed → came | no_show; cancelled; waitlist (wanted, not yet placed)
    status           TEXT NOT NULL DEFAULT 'booked' CHECK (status IN ('booked', 'confirmed', 'came', 'no_show', 'cancelled', 'waitlist')),
    service_id       UUID REFERENCES booking_services (id) ON DELETE SET NULL,
    extra_ids        UUID[] NOT NULL DEFAULT '{}',
    price            NUMERIC(14, 2) NOT NULL DEFAULT 0,
    client_name      TEXT NOT NULL,
    client_email     TEXT,
    client_phone     TEXT,
    contact_id       UUID REFERENCES crm_items (id) ON DELETE SET NULL,
    deal_id          UUID REFERENCES crm_items (id) ON DELETE SET NULL,
    note             TEXT,
    -- manual | form
    source           TEXT NOT NULL DEFAULT 'manual',
    form_id          UUID,
    -- Over a resource's capacity, by staff.
    overbooked       BOOLEAN NOT NULL DEFAULT FALSE,
    -- The client's link to see, confirm or cancel it (only its hash is kept).
    token_hash       TEXT UNIQUE,
    reminded_at      TIMESTAMPTZ,
    created_by       UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (ends_at > starts_at)
);
CREATE INDEX bookings_time_idx ON bookings (starts_at ASC, ends_at ASC);
CREATE INDEX bookings_contact_idx ON bookings (contact_id ASC) WHERE contact_id IS NOT NULL;

-- The resources a booking takes (several at once: a room and a person).
CREATE TABLE booking_resources_used (
    booking_id  UUID NOT NULL REFERENCES bookings (id) ON DELETE CASCADE,
    resource_id UUID NOT NULL REFERENCES booking_resources (id) ON DELETE CASCADE,
    PRIMARY KEY (booking_id, resource_id)
);
CREATE INDEX booking_resources_used_res_idx ON booking_resources_used (resource_id ASC);

-- Self-booking forms at /book/<slug>.
CREATE TABLE booking_forms (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name          TEXT NOT NULL,
    slug          TEXT NOT NULL UNIQUE,
    intro         TEXT,
    -- What can be booked through it (empty: every active one).
    service_ids   UUID[] NOT NULL DEFAULT '{}',
    resource_ids  UUID[] NOT NULL DEFAULT '{}',
    ask_phone     BOOLEAN NOT NULL DEFAULT TRUE,
    phone_required BOOLEAN NOT NULL DEFAULT FALSE,
    -- Clients may choose who (or what) they book; otherwise the first free one.
    choose_resource BOOLEAN NOT NULL DEFAULT TRUE,
    -- Join the waiting list when nothing is free.
    waitlist      BOOLEAN NOT NULL DEFAULT TRUE,
    thanks        TEXT,
    is_active     BOOLEAN NOT NULL DEFAULT TRUE,
    created_by    UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);
