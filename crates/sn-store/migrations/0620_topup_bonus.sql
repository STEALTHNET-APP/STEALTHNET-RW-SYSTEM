ALTER TABLE balance_entries DROP CONSTRAINT balance_entries_kind_check;
ALTER TABLE balance_entries ADD CONSTRAINT balance_entries_kind_check CHECK (kind IN ('topup', 'purchase', 'autorenew', 'refund', 'referral_credit', 'referral_reversal', 'withdrawal_hold', 'withdrawal_release', 'contest', 'roulette', 'admin_adjust', 'migration_opening', 'refund_hold', 'refund_release', 'topup_bonus', 'topup_bonus_reversal'));
ALTER TABLE payments ADD COLUMN bonus_terms jsonb;
ALTER TABLE payments ADD COLUMN bonus_minor bigint NOT NULL DEFAULT 0 CHECK (bonus_minor >= 0);
ALTER TABLE refunds ADD COLUMN bonus_minor bigint NOT NULL DEFAULT 0 CHECK (bonus_minor >= 0);
ALTER TABLE provider_refunds ADD COLUMN bonus_minor bigint NOT NULL DEFAULT 0 CHECK (bonus_minor >= 0);
