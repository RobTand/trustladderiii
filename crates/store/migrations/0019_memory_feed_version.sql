-- #773 / Q7: clearance does not manufacture a new failed attempt.
ALTER TABLE tl3_memory_feed ADD COLUMN version bigint NOT NULL DEFAULT 0;
