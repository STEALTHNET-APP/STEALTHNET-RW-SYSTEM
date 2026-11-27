ALTER TABLE provider_subscriptions ALTER COLUMN first_payment_id DROP NOT NULL;
ALTER TABLE provider_subscriptions ADD COLUMN legacy_invoice_id bigint REFERENCES legacy_invoices (id) ON DELETE SET NULL;
ALTER TABLE provider_subscriptions ADD COLUMN charges_since timestamptz;
ALTER TABLE provider_subscriptions ADD COLUMN started_at timestamptz;

ALTER TABLE legacy_invoices ADD COLUMN contract_id text;
ALTER TABLE legacy_invoices ADD COLUMN contract_state text CHECK (contract_state IN ('linked', 'unbound', 'conflict', 'ended', 'missing', 'error'));
ALTER TABLE legacy_invoices ADD COLUMN contract_check_at timestamptz;
ALTER TABLE legacy_invoices ADD COLUMN contract_checked_at timestamptz;

CREATE INDEX legacy_invoices_contract_check_idx ON legacy_invoices (contract_check_at NULLS FIRST, id)
    WHERE recurring AND provider = 'lava';
