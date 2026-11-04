-- #773: runtime health and durable retry evidence, outside kernel revisions.
CREATE TABLE tl3_memory_feed (
    singleton boolean PRIMARY KEY DEFAULT true CHECK (singleton),
    state text NOT NULL CHECK (state IN ('delivering', 'ready', 'held')),
    retry bigint NOT NULL DEFAULT 0
);
CREATE TRIGGER tl3_memory_feed_changes AFTER INSERT OR UPDATE ON tl3_memory_feed
    FOR EACH ROW EXECUTE FUNCTION tl3_notify_change();
