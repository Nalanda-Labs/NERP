-- Tasks and calendar events get a chat of their own (made on first use, like a workgroup's),
-- whose members follow the people on the task or event; a call in it is the task's or the
-- meeting's call.
ALTER TABLE im_chats ADD COLUMN task_id UUID UNIQUE REFERENCES tasks (id) ON DELETE CASCADE;
ALTER TABLE im_chats ADD COLUMN event_id UUID UNIQUE REFERENCES cal_events (id) ON DELETE CASCADE;
