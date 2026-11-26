ALTER TABLE referral_links
    ADD COLUMN set_by bigint REFERENCES admins (id),
    ADD COLUMN set_at timestamptz;
