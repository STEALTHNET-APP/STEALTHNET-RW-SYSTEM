CREATE TABLE tariff_promotions (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    name text NOT NULL CHECK (btrim(name) <> '' AND char_length(name) <= 40),
    percent smallint NOT NULL CHECK (percent BETWEEN 1 AND 90),
    tariff_ids bigint[] NOT NULL CHECK (cardinality(tariff_ids) BETWEEN 1 AND 200),
    starts_at timestamptz NOT NULL,
    ends_at timestamptz NOT NULL,
    enabled boolean NOT NULL DEFAULT true,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CHECK (ends_at > starts_at)
);

CREATE INDEX tariff_promotions_live_idx ON tariff_promotions (ends_at) WHERE enabled;
