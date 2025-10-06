-- Add payment_request and payment_proof to transactions table
ALTER TABLE transactions ADD COLUMN payment_request TEXT;
ALTER TABLE transactions ADD COLUMN payment_proof TEXT;
