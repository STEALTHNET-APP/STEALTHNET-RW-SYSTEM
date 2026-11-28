ALTER TABLE broadcast_images DROP CONSTRAINT broadcast_images_content_type_check;
ALTER TABLE broadcast_images DROP CONSTRAINT broadcast_images_bytes_check;

ALTER TABLE broadcast_images ADD COLUMN kind text NOT NULL DEFAULT 'photo';
ALTER TABLE broadcast_images ADD COLUMN duration integer;
ALTER TABLE broadcast_images ADD COLUMN streaming boolean NOT NULL DEFAULT false;

ALTER TABLE broadcast_images ADD CONSTRAINT broadcast_images_kind_check CHECK (
    (kind = 'photo'
        AND content_type IN ('image/png', 'image/jpeg')
        AND octet_length(bytes) BETWEEN 1 AND 10000000
        AND duration IS NULL
        AND NOT streaming)
    OR (kind = 'video'
        AND content_type = 'video/mp4'
        AND octet_length(bytes) BETWEEN 1 AND 50000000
        AND duration IS NOT NULL
        AND duration >= 0)
);

ALTER TABLE broadcast_images ALTER COLUMN bytes SET STORAGE EXTERNAL;
