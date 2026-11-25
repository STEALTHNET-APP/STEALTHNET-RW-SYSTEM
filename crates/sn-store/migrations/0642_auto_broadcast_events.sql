CREATE INDEX orders_unpaid_created_idx ON orders (created_at)
    WHERE paid_at IS NULL AND kind IN ('new', 'renewal', 'change');

CREATE INDEX orders_bought_paid_idx ON orders (paid_at)
    WHERE paid_at IS NOT NULL AND kind IN ('new', 'renewal', 'change');

CREATE INDEX subscriptions_online_idx ON subscriptions (online_at)
    WHERE ended_at IS NULL AND online_at IS NOT NULL;

CREATE INDEX subscriptions_devices_full_idx ON subscriptions (id)
    WHERE ended_at IS NULL AND hwid_limit > 0 AND devices_count >= hwid_limit;

ALTER TABLE auto_broadcasts DROP CONSTRAINT auto_broadcasts_trigger_check;
ALTER TABLE auto_broadcasts ADD CONSTRAINT auto_broadcasts_trigger_check CHECK (
    trigger IN ('expiring', 'expired', 'trial_ended', 'no_purchase', 'not_connected', 'no_subscription',
                'trial_expiring', 'traffic_low', 'unpaid_order', 'first_purchase', 'idle', 'devices_full')
);

ALTER TABLE auto_broadcasts DROP CONSTRAINT auto_broadcasts_percent_check;
ALTER TABLE auto_broadcasts ADD CONSTRAINT auto_broadcasts_percent_check CHECK (percent BETWEEN 10 AND 100);
