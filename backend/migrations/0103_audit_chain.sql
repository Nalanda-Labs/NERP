-- Tamper-evident audit trail and full company export.
--
-- Each audit record is sealed into a hash chain (chain_pos, prev_hash, hash) by the
-- audit.seal job. `actor` keeps who did it even after the employee is deleted (actor_id is
-- set to NULL then), so the chain still verifies. Records can't be changed or removed once
-- written; only sealing (once) and the actor_id → NULL on deletion are allowed.
ALTER TABLE audit_log
    ADD COLUMN actor     UUID,
    ADD COLUMN chain_pos BIGINT UNIQUE,
    ADD COLUMN prev_hash BYTEA,
    ADD COLUMN hash      BYTEA;
UPDATE audit_log SET actor = actor_id;
CREATE INDEX audit_log_unsealed_idx ON audit_log (created_at, id) WHERE chain_pos IS NULL;

CREATE FUNCTION audit_log_guard() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'INSERT' THEN
        NEW.actor := NEW.actor_id;
        NEW.chain_pos := NULL;
        NEW.prev_hash := NULL;
        NEW.hash := NULL;
        RETURN NEW;
    ELSIF TG_OP = 'UPDATE' THEN
        IF NEW.id = OLD.id
           AND NEW.actor IS NOT DISTINCT FROM OLD.actor
           AND NEW.action = OLD.action
           AND NEW.entity_type = OLD.entity_type
           AND NEW.entity_id IS NOT DISTINCT FROM OLD.entity_id
           AND NEW.details = OLD.details
           AND NEW.ip IS NOT DISTINCT FROM OLD.ip
           AND NEW.created_at = OLD.created_at
           AND (NEW.actor_id IS NOT DISTINCT FROM OLD.actor_id OR NEW.actor_id IS NULL)
           AND (OLD.chain_pos IS NULL
                OR (NEW.chain_pos = OLD.chain_pos AND NEW.hash = OLD.hash
                    AND NEW.prev_hash IS NOT DISTINCT FROM OLD.prev_hash)) THEN
            RETURN NEW;
        END IF;
        RAISE EXCEPTION 'audit records cannot be changed';
    END IF;
    RAISE EXCEPTION 'audit records cannot be removed';
END $$;

CREATE TRIGGER audit_log_guard BEFORE INSERT OR UPDATE OR DELETE ON audit_log
    FOR EACH ROW EXECUTE FUNCTION audit_log_guard();
CREATE TRIGGER audit_log_no_truncate BEFORE TRUNCATE ON audit_log
    FOR EACH STATEMENT EXECUTE FUNCTION audit_log_guard();

-- Signed checkpoints of the chain's head. The signature uses a key derived from the server's
-- DATA_ENCRYPTION_KEY, which isn't in the database: rewriting the chain from the database
-- alone can't produce matching checkpoints.
CREATE TABLE audit_anchors (
    id         BIGSERIAL PRIMARY KEY,
    chain_pos  BIGINT NOT NULL,
    hash       BYTEA NOT NULL,
    mac        BYTEA NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE FUNCTION audit_anchors_guard() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'audit checkpoints cannot be changed or removed';
END $$;
CREATE TRIGGER audit_anchors_guard BEFORE UPDATE OR DELETE ON audit_anchors
    FOR EACH ROW EXECUTE FUNCTION audit_anchors_guard();
CREATE TRIGGER audit_anchors_no_truncate BEFORE TRUNCATE ON audit_anchors
    FOR EACH STATEMENT EXECUTE FUNCTION audit_anchors_guard();

-- Full company exports (a ZIP of every table as CSV and the stored files).
CREATE TABLE company_exports (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    status        TEXT NOT NULL DEFAULT 'running' CHECK (status IN ('running', 'done', 'failed')),
    include_files BOOLEAN NOT NULL DEFAULT TRUE,
    requested_by  UUID REFERENCES users (id) ON DELETE SET NULL,
    file_name     TEXT,
    size_bytes    BIGINT,
    sha256        TEXT,
    tables        INT,
    files         INT,
    error         TEXT,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at   TIMESTAMPTZ
);
