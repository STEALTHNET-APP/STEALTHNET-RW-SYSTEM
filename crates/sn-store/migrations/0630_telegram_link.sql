ALTER TABLE client_login_requests ADD COLUMN session_id bigint REFERENCES client_sessions (id) ON DELETE CASCADE;
ALTER TABLE client_login_requests DROP CONSTRAINT client_login_requests_method_check;
ALTER TABLE client_login_requests ADD CONSTRAINT client_login_requests_method_check
    CHECK (method IN ('telegram', 'email', 'totp', 'reset', 'email_change', 'email_proof', 'telegram_link'));
ALTER TABLE client_login_requests ADD CONSTRAINT client_login_requests_telegram_link_check
    CHECK (method <> 'telegram_link' OR (client_id IS NOT NULL AND session_id IS NOT NULL AND poll_hash IS NOT NULL));
CREATE INDEX client_login_requests_telegram_link_idx ON client_login_requests (client_id)
    WHERE method = 'telegram_link' AND status = 'pending';
CREATE INDEX client_login_requests_session_idx ON client_login_requests (session_id)
    WHERE session_id IS NOT NULL;
