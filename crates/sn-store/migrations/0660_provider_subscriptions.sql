CREATE TABLE provider_subscriptions (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    provider text NOT NULL,
    external_id text NOT NULL,
    client_id bigint NOT NULL REFERENCES clients (id),
    subscription_id bigint REFERENCES subscriptions (id),
    first_payment_id bigint NOT NULL REFERENCES payments (id),
    tariff_id bigint,
    period_id bigint,
    days integer NOT NULL CHECK (days > 0),
    offer_id text NOT NULL,
    product_id text,
    periodicity text NOT NULL,
    every_text text NOT NULL DEFAULT '',
    name text NOT NULL DEFAULT '',
    amount_minor bigint NOT NULL CHECK (amount_minor > 0),
    currency text NOT NULL,
    email text NOT NULL,
    status text NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'active', 'cancelling', 'cancelled', 'failed')),
    cancel_source text CHECK (cancel_source IN ('client', 'operator', 'provider', 'refund', 'tariff', 'block', 'chargeback', 'gone')),
    cancel_requested_at timestamptz,
    cancel_attempts integer NOT NULL DEFAULT 0,
    cancel_error text,
    ended_at timestamptz,
    paid_until timestamptz,
    next_charge_at timestamptz,
    check_at timestamptz,
    checked_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT provider_subscriptions_external_key UNIQUE (provider, external_id)
);

CREATE UNIQUE INDEX provider_subscriptions_live_key ON provider_subscriptions (subscription_id) WHERE status IN ('active', 'cancelling');
CREATE INDEX provider_subscriptions_check_idx ON provider_subscriptions (check_at) WHERE status IN ('active', 'cancelling');
CREATE INDEX provider_subscriptions_client_idx ON provider_subscriptions (client_id);
CREATE INDEX provider_subscriptions_payment_idx ON provider_subscriptions (first_payment_id);

CREATE TABLE provider_subscription_charges (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    provider_subscription_id bigint NOT NULL REFERENCES provider_subscriptions (id),
    payment_id bigint NOT NULL REFERENCES payments (id),
    order_id bigint NOT NULL REFERENCES orders (id),
    external_id text NOT NULL,
    first boolean NOT NULL DEFAULT false,
    charged_at timestamptz NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT provider_subscription_charges_payment_key UNIQUE (payment_id),
    CONSTRAINT provider_subscription_charges_external_key UNIQUE (provider_subscription_id, external_id)
);

CREATE INDEX provider_subscription_charges_order_idx ON provider_subscription_charges (order_id);
