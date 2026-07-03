-- Enforce the ADR's append-only invariant on mint_event_log at the
-- database level (ADR "Invariants" item 2), instead of relying on code
-- review alone:
--
--  * DELETE is never allowed.
--  * The only allowed UPDATE is the appender's one-time leaf_index
--    assignment: leaf_index NULL -> value, every other column unchanged.
--
-- The hash-covered columns (entity_type, entity_id, op, payload,
-- leaf_hash, created_time) and the row id therefore become immutable the
-- moment the row is inserted.

CREATE FUNCTION mint_event_log_append_only_guard() RETURNS trigger AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        RAISE EXCEPTION 'mint_event_log is append-only; DELETE is not allowed';
    END IF;

    IF OLD.leaf_index IS NOT NULL
        OR NEW.leaf_index IS NULL
        OR NEW.seq IS DISTINCT FROM OLD.seq
        OR NEW.entity_type IS DISTINCT FROM OLD.entity_type
        OR NEW.entity_id IS DISTINCT FROM OLD.entity_id
        OR NEW.op IS DISTINCT FROM OLD.op
        OR NEW.payload IS DISTINCT FROM OLD.payload
        OR NEW.leaf_hash IS DISTINCT FROM OLD.leaf_hash
        OR NEW.created_time IS DISTINCT FROM OLD.created_time
    THEN
        RAISE EXCEPTION 'mint_event_log is append-only; only one-time leaf_index assignment is allowed';
    END IF;

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER mint_event_log_append_only
BEFORE UPDATE OR DELETE ON mint_event_log
FOR EACH ROW EXECUTE FUNCTION mint_event_log_append_only_guard();
