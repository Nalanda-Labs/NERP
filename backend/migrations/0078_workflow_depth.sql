-- Workflow depth: tasks a process waits for, test mode (only chosen people's items start
-- the process), and feed posts that are requests (a list item started from the feed).
CREATE TABLE wf_tasks (
    instance_id UUID NOT NULL REFERENCES wf_instances (id) ON DELETE CASCADE,
    task_id     UUID NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
    PRIMARY KEY (instance_id, task_id)
);
CREATE INDEX wf_tasks_task_idx ON wf_tasks (task_id);

ALTER TABLE wf_templates ADD COLUMN test_users UUID[] NOT NULL DEFAULT '{}';

ALTER TABLE feed_posts ADD COLUMN list_item_id UUID REFERENCES list_items (id) ON DELETE SET NULL;
