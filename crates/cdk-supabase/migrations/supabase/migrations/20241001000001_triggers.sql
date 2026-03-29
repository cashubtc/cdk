-- Attach optimistic-locking triggers (depends on increment_opt_version)
CREATE TRIGGER increment_mint_opt_version
    BEFORE UPDATE ON mint
    FOR EACH ROW EXECUTE FUNCTION increment_opt_version();

CREATE TRIGGER increment_proof_opt_version
    BEFORE UPDATE ON proof
    FOR EACH ROW EXECUTE FUNCTION increment_opt_version();

CREATE TRIGGER increment_transactions_opt_version
    BEFORE UPDATE ON transactions
    FOR EACH ROW EXECUTE FUNCTION increment_opt_version();
