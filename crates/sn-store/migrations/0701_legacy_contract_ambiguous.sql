ALTER TABLE legacy_invoices DROP CONSTRAINT legacy_invoices_contract_state_check;
ALTER TABLE legacy_invoices ADD CONSTRAINT legacy_invoices_contract_state_check
    CHECK (contract_state IN ('linked', 'unbound', 'conflict', 'ambiguous', 'ended', 'missing', 'error'));

UPDATE legacy_invoices SET contract_check_at = now()
    WHERE contract_state = 'missing' AND contract_check_at IS NULL;
