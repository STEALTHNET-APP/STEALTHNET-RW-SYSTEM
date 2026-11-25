CREATE INDEX subscriptions_tariff_live_idx ON subscriptions (tariff_id) WHERE ended_at IS NULL AND tariff_id IS NOT NULL;
