CREATE TABLE bot_maintenance_notices (
    chat_id bigint PRIMARY KEY,
    sent_at timestamptz NOT NULL DEFAULT now()
);
