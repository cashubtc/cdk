-- Publish committed state transitions, including those written by older
-- replicas during a rolling restart. The channel is scoped to the mint schema.
CREATE FUNCTION cdk_notify_proof_state() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        PERFORM pg_catalog.pg_notify('cdk_mint_' || pg_catalog.md5(TG_TABLE_SCHEMA),
            'proof:' || pg_catalog.encode(OLD.y, 'hex') || ':UNSPENT');
    ELSE
        PERFORM pg_catalog.pg_notify('cdk_mint_' || pg_catalog.md5(TG_TABLE_SCHEMA),
            'proof:' || pg_catalog.encode(NEW.y, 'hex') || ':' || NEW.state);
    END IF;
    RETURN NULL;
END
$$;

CREATE TRIGGER cdk_proof_state_changed
AFTER UPDATE OF state ON proof
FOR EACH ROW WHEN (OLD.state IS DISTINCT FROM NEW.state)
EXECUTE FUNCTION cdk_notify_proof_state();

CREATE TRIGGER cdk_proof_state_inserted
AFTER INSERT ON proof
FOR EACH ROW WHEN (NEW.state <> 'UNSPENT')
EXECUTE FUNCTION cdk_notify_proof_state();

CREATE TRIGGER cdk_proof_state_deleted
AFTER DELETE ON proof
FOR EACH ROW WHEN (OLD.state <> 'UNSPENT')
EXECUTE FUNCTION cdk_notify_proof_state();

CREATE FUNCTION cdk_notify_mint_quote() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    PERFORM pg_catalog.pg_notify('cdk_mint_' || pg_catalog.md5(TG_TABLE_SCHEMA),
        'mint:' || NEW.id || ':' || NEW.amount_paid || ':' || NEW.amount_issued);
    RETURN NULL;
END
$$;

CREATE TRIGGER cdk_mint_quote_changed
AFTER UPDATE OF amount_paid, amount_issued ON mint_quote
FOR EACH ROW WHEN (
    OLD.amount_paid IS DISTINCT FROM NEW.amount_paid OR
    OLD.amount_issued IS DISTINCT FROM NEW.amount_issued
)
EXECUTE FUNCTION cdk_notify_mint_quote();

CREATE FUNCTION cdk_notify_melt_quote() RETURNS trigger
LANGUAGE plpgsql SET search_path FROM CURRENT AS $$
BEGIN
    -- A paid quote is still pending to clients until change is persisted.
    IF NEW.state <> 'PAID' OR NOT EXISTS (
        SELECT 1 FROM melt_request WHERE quote_id = NEW.id
    ) THEN
        PERFORM pg_catalog.pg_notify('cdk_mint_' || pg_catalog.md5(TG_TABLE_SCHEMA),
            'melt:' || NEW.id || ':' || NEW.state);
    END IF;
    RETURN NULL;
END
$$;

CREATE TRIGGER cdk_melt_quote_changed
AFTER UPDATE OF state ON melt_quote
FOR EACH ROW WHEN (OLD.state IS DISTINCT FROM NEW.state)
EXECUTE FUNCTION cdk_notify_melt_quote();

CREATE FUNCTION cdk_notify_finalized_melt() RETURNS trigger
LANGUAGE plpgsql SET search_path FROM CURRENT AS $$
BEGIN
    IF EXISTS (SELECT 1 FROM melt_quote WHERE id = OLD.quote_id AND state = 'PAID') THEN
        PERFORM pg_catalog.pg_notify('cdk_mint_' || pg_catalog.md5(TG_TABLE_SCHEMA),
            'melt:' || OLD.quote_id || ':PAID');
    END IF;
    RETURN NULL;
END
$$;

CREATE TRIGGER cdk_melt_request_completed
AFTER DELETE ON melt_request
FOR EACH ROW EXECUTE FUNCTION cdk_notify_finalized_melt();
