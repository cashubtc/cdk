-- CDK Wallet Database Schema Migration 002: Atomic Keyset Counter
-- This migration adds an RPC function for atomic counter increments

-- Function: increment_keyset_counter
-- Atomically increments the keyset counter and returns the new value
-- Uses INSERT ... ON CONFLICT to handle upserts atomically
CREATE OR REPLACE FUNCTION increment_keyset_counter(
    p_keyset_id TEXT,
    p_increment INTEGER DEFAULT 1
)
RETURNS INTEGER
LANGUAGE plpgsql
SECURITY DEFINER
AS $$
DECLARE
    new_counter INTEGER;
BEGIN
    INSERT INTO keyset_counter (keyset_id, counter)
    VALUES (p_keyset_id, p_increment)
    ON CONFLICT (keyset_id)
    DO UPDATE SET counter = keyset_counter.counter + p_increment
    RETURNING counter INTO new_counter;
    
    RETURN new_counter;
END;
$$;

-- Grant execute permission
GRANT EXECUTE ON FUNCTION increment_keyset_counter(TEXT, INTEGER) TO authenticated;
GRANT EXECUTE ON FUNCTION increment_keyset_counter(TEXT, INTEGER) TO service_role;

-- Add a comment describing the function
COMMENT ON FUNCTION increment_keyset_counter(TEXT, INTEGER) IS 
    'Atomically increments the keyset counter by the specified amount and returns the new value. '
    'Creates the counter with the increment value if it does not exist.';
