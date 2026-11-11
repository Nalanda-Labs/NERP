-- Automation test mode: while on, a pipeline's robots, triggers and scheduled robots run only
-- for records whose responsible person is one of the testers. Runs made then are marked.
ALTER TABLE crm_categories
    ADD COLUMN automation_test_mode BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN automation_testers uuid[] NOT NULL DEFAULT '{}';
ALTER TABLE crm_robot_runs ADD COLUMN is_test BOOLEAN NOT NULL DEFAULT FALSE;
