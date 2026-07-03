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

CREATE TRIGGER mint_event_log_no_delete
BEFORE DELETE ON mint_event_log
BEGIN
    SELECT RAISE(ABORT, 'mint_event_log is append-only; DELETE is not allowed');
END;

CREATE TRIGGER mint_event_log_no_update
BEFORE UPDATE ON mint_event_log
WHEN NOT (
    OLD.leaf_index IS NULL
    AND NEW.leaf_index IS NOT NULL
    AND NEW.seq = OLD.seq
    AND NEW.entity_type = OLD.entity_type
    AND NEW.entity_id = OLD.entity_id
    AND NEW.op = OLD.op
    AND NEW.payload = OLD.payload
    AND NEW.leaf_hash = OLD.leaf_hash
    AND NEW.created_time = OLD.created_time
)
BEGIN
    SELECT RAISE(ABORT, 'mint_event_log is append-only; only one-time leaf_index assignment is allowed');
END;
