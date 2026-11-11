-- The shop: coupons, discounts for loyal customers (by what they bought before), and ways
-- of delivery with their price (free above an amount if wanted).
CREATE TABLE shop_coupons (
    id                UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    -- Upper case.
    code              TEXT NOT NULL UNIQUE,
    kind              TEXT NOT NULL CHECK (kind IN ('percent', 'amount')),
    value             NUMERIC(24, 4) NOT NULL CHECK (value > 0),
    min_total         NUMERIC(24, 4),
    starts_at         TIMESTAMPTZ,
    ends_at           TIMESTAMPTZ,
    max_uses          INT CHECK (max_uses IS NULL OR max_uses > 0),
    uses              INT NOT NULL DEFAULT 0,
    once_per_customer BOOLEAN NOT NULL DEFAULT FALSE,
    active            BOOLEAN NOT NULL DEFAULT TRUE,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE shop_coupon_uses (
    coupon_id UUID NOT NULL REFERENCES shop_coupons (id) ON DELETE CASCADE,
    email     TEXT NOT NULL,
    item_id   UUID REFERENCES crm_items (id) ON DELETE SET NULL,
    used_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX shop_coupon_uses_idx ON shop_coupon_uses (coupon_id, email);

CREATE TABLE shop_delivery (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name        TEXT NOT NULL,
    description TEXT,
    price       NUMERIC(24, 4) NOT NULL DEFAULT 0 CHECK (price >= 0),
    free_from   NUMERIC(24, 4),
    -- Pick-up and the like need no address.
    needs_address BOOLEAN NOT NULL DEFAULT TRUE,
    sort        INT NOT NULL DEFAULT 0,
    active      BOOLEAN NOT NULL DEFAULT TRUE
);

-- [{"from": 1000, "percent": 5}, …]: what a customer bought before (by email), in the shop's currency.
ALTER TABLE shop_settings ADD COLUMN loyalty JSONB NOT NULL DEFAULT '[]';
-- What an order got: {"coupon": "SPRING", "discount_percent": 10, "delivery": "Courier", "delivery_price": 5}
ALTER TABLE shop_orders ADD COLUMN extras JSONB NOT NULL DEFAULT '{}';
