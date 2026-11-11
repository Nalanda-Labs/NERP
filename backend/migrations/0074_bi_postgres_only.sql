-- BI reads outside PostgreSQL databases only: MySQL connections are gone, with the datasets
-- made from them and the charts that showed those datasets.
DELETE FROM bi_widgets
 WHERE spec->>'dataset' IN (
       SELECT d.id::text FROM bi_datasets d JOIN bi_sources s ON s.id = d.source_id WHERE s.kind = 'mysql');
DELETE FROM bi_sources WHERE kind = 'mysql';

ALTER TABLE bi_sources DROP CONSTRAINT bi_sources_kind_check;
ALTER TABLE bi_sources ADD CONSTRAINT bi_sources_kind_check CHECK (kind = 'postgres');
