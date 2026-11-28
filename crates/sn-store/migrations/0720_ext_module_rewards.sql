CREATE TABLE ext_module_rewards (
    module_id TEXT NOT NULL,
    reference TEXT NOT NULL,
    client_id BIGINT NOT NULL,
    request JSONB NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('applying', 'applied', 'failed')),
    result JSONB,
    error TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (module_id, reference)
);

CREATE INDEX ext_module_rewards_client_idx ON ext_module_rewards (client_id, created_at DESC);
