ALTER TABLE auto_broadcasts ADD COLUMN audience jsonb CHECK (audience IS NULL OR jsonb_typeof(audience) = 'object');
