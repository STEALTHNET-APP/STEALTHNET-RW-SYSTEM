ALTER TABLE clients ADD COLUMN receipt_email text
    CONSTRAINT clients_receipt_email_check CHECK (receipt_email IS NULL OR (length(receipt_email) BETWEEN 3 AND 254 AND position('@' IN receipt_email) > 1));
