-- Add payment_method to transactions table
ALTER TABLE transactions ADD COLUMN payment_method TEXT;
