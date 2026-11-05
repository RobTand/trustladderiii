-- #773 / Q7: an outcome belongs to the operation attempted, not the current queue head.
-- Pre-upgrade outcomes keep the exact legacy scope instead of borrowing a new identity.
ALTER TABLE tl3_memory_feed ADD COLUMN lineage text NOT NULL DEFAULT 'memory';
