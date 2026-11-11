-- A meeting a client booked through a slot link sent from a CRM record: the record's meeting
-- activity points at the calendar event (the calendar shows the event, not both).
ALTER TABLE crm_activities ADD COLUMN cal_event_id UUID REFERENCES cal_events (id) ON DELETE SET NULL;
CREATE INDEX crm_activities_event_idx ON crm_activities (cal_event_id) WHERE cal_event_id IS NOT NULL;
