-- Migration 003: Add mint_url to melt_quote
-- Supports tracking which mint a melt quote belongs to

ALTER TABLE melt_quote ADD COLUMN IF NOT EXISTS mint_url TEXT;
