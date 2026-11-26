ALTER TABLE tickets ADD COLUMN last_message_at timestamptz;

ALTER TABLE tickets DISABLE TRIGGER tickets_touch;
UPDATE tickets t SET last_message_at = m.at
FROM (SELECT ticket_id, max(created_at) AS at FROM ticket_messages GROUP BY ticket_id) m
WHERE m.ticket_id = t.id;
ALTER TABLE tickets ENABLE TRIGGER tickets_touch;

CREATE INDEX tickets_recent_idx ON tickets (status, (COALESCE(last_message_at, created_at)) DESC, id DESC);
CREATE INDEX tickets_waiting_recent_idx ON tickets ((COALESCE(last_message_at, created_at)) DESC, id DESC)
    WHERE status = 'open' AND last_author = 'client';

DROP INDEX tickets_queue_idx;
DROP INDEX tickets_waiting_queue_idx;

ANALYZE tickets;
