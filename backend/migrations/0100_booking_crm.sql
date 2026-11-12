-- Booking inside CRM: more reminders, a follow-up after the visit, a message to no-shows
-- (each sent once: booking_notices), and booking events as CRM automation triggers.
ALTER TABLE booking_settings ADD COLUMN extra_reminders INT[] NOT NULL DEFAULT '{}';
ALTER TABLE booking_settings ADD COLUMN follow_up_hours INT NOT NULL DEFAULT 0 CHECK (follow_up_hours BETWEEN 0 AND 720);
ALTER TABLE booking_settings ADD COLUMN follow_up_text TEXT;
ALTER TABLE booking_settings ADD COLUMN no_show_text TEXT;

CREATE TABLE booking_notices (
    booking_id UUID NOT NULL REFERENCES bookings (id) ON DELETE CASCADE,
    -- remind:<hours> | follow_up | no_show
    kind       TEXT NOT NULL,
    sent_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (booking_id, kind)
);
-- Reminders already sent under the single setting.
INSERT INTO booking_notices (booking_id, kind, sent_at)
SELECT b.id, 'remind:' || s.remind_hours, b.reminded_at FROM bookings b, booking_settings s WHERE b.reminded_at IS NOT NULL AND s.remind_hours > 0
ON CONFLICT DO NOTHING;
