-- Accounting: double-entry bookkeeping. A chart of accounts; a journal of entries whose lines
-- always balance; sales invoices (the CRM's) and purchase bills posted to it; payments
-- allocated to them; bank statements imported and reconciled; taxes; a lock date and year-end
-- closing. Posted entries never change: corrections are reversing entries.
--
-- Amounts in the journal are in the base currency (the CRM's), rounded to 2 decimals; lines of
-- foreign-currency documents also keep the amount in that currency.

CREATE TABLE acc_accounts (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    code        TEXT NOT NULL UNIQUE,
    name        TEXT NOT NULL,
    -- asset | liability | equity | income | expense
    type        TEXT NOT NULL CHECK (type IN ('asset', 'liability', 'equity', 'income', 'expense')),
    -- What kind within the type; bank and cash accounts can be reconciled and receive payments.
    subtype     TEXT NOT NULL CHECK (subtype IN (
        'bank', 'cash', 'receivable', 'current_asset', 'inventory', 'fixed_asset', 'tax_receivable',
        'payable', 'current_liability', 'tax_payable', 'long_term_liability',
        'equity', 'retained_earnings',
        'income', 'other_income',
        'cost_of_sales', 'expense', 'other_expense')),
    parent_id   UUID REFERENCES acc_accounts (id) ON DELETE SET NULL,
    -- Bank accounts kept in another currency; NULL: the base currency.
    currency    CHAR(3),
    description TEXT,
    is_active   BOOLEAN NOT NULL DEFAULT TRUE,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- One row: how the books are kept.
CREATE TABLE acc_settings (
    id                   INT PRIMARY KEY DEFAULT 1 CHECK (id = 1),
    -- Set up (chart of accounts made); nothing can be posted before.
    ready                BOOLEAN NOT NULL DEFAULT FALSE,
    -- The month the financial year starts (1 = January).
    fiscal_year_start    SMALLINT NOT NULL DEFAULT 1 CHECK (fiscal_year_start BETWEEN 1 AND 12),
    -- Nothing dated on or before this can be posted, changed or voided.
    lock_date            DATE,
    receivable_account   UUID REFERENCES acc_accounts (id),
    payable_account      UUID REFERENCES acc_accounts (id),
    sales_account        UUID REFERENCES acc_accounts (id),
    retained_earnings    UUID REFERENCES acc_accounts (id),
    opening_balances     UUID REFERENCES acc_accounts (id),
    fx_account           UUID REFERENCES acc_accounts (id),
    rounding_account     UUID REFERENCES acc_accounts (id),
    -- Move a CRM invoice to its "paid" stage when it is fully paid.
    mark_invoices_paid   BOOLEAN NOT NULL DEFAULT TRUE
);
INSERT INTO acc_settings (id) VALUES (1);

CREATE TABLE acc_taxes (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name       TEXT NOT NULL,
    rate       NUMERIC(7, 4) NOT NULL CHECK (rate >= 0 AND rate <= 100),
    -- sales: charged on invoices (owed); purchases: paid on bills (reclaimable)
    scope      TEXT NOT NULL CHECK (scope IN ('sales', 'purchases')),
    account_id UUID NOT NULL REFERENCES acc_accounts (id),
    is_active  BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Gapless numbers per kind (entries, bills, payments).
CREATE TABLE acc_counters (
    kind TEXT PRIMARY KEY,
    next BIGINT NOT NULL DEFAULT 1
);
INSERT INTO acc_counters (kind) VALUES ('entry'), ('bill'), ('payment');

CREATE TABLE acc_entries (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    -- Given when posted.
    number      BIGINT UNIQUE,
    date        DATE NOT NULL,
    memo        TEXT,
    -- manual | invoice | bill | payment | bank | opening | closing
    source      TEXT NOT NULL CHECK (source IN ('manual', 'invoice', 'bill', 'payment', 'bank', 'opening', 'closing')),
    source_id   UUID,
    -- draft (manual entries only) | posted | void (reversed)
    status      TEXT NOT NULL DEFAULT 'posted' CHECK (status IN ('draft', 'posted', 'void')),
    -- A reversing entry points at the entry it reverses, and that one back at it.
    reverses    UUID REFERENCES acc_entries (id),
    reversed_by UUID REFERENCES acc_entries (id),
    created_by  UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    posted_at   TIMESTAMPTZ
);
CREATE INDEX acc_entries_date_idx ON acc_entries (date DESC, number DESC);
CREATE INDEX acc_entries_source_idx ON acc_entries (source, source_id);

CREATE TABLE acc_lines (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    entry_id        UUID NOT NULL REFERENCES acc_entries (id) ON DELETE CASCADE,
    account_id      UUID NOT NULL REFERENCES acc_accounts (id),
    -- The entry's date (for reports by account and period).
    date            DATE NOT NULL,
    debit           NUMERIC(20, 2) NOT NULL DEFAULT 0 CHECK (debit >= 0),
    credit          NUMERIC(20, 2) NOT NULL DEFAULT 0 CHECK (credit >= 0),
    -- Foreign-currency documents: the signed amount in that currency (debit positive).
    currency        CHAR(3),
    amount_currency NUMERIC(20, 2),
    description     TEXT,
    -- The customer or supplier (a CRM company or contact), for receivables and payables.
    party_id        UUID REFERENCES crm_items (id) ON DELETE SET NULL,
    tax_id          UUID REFERENCES acc_taxes (id),
    -- Sales and purchases this tax line is on (for the tax report).
    tax_base        NUMERIC(20, 2),
    sort            INT NOT NULL DEFAULT 0,
    CHECK (debit = 0 OR credit = 0)
);
CREATE INDEX acc_lines_entry_idx ON acc_lines (entry_id ASC, sort ASC);
CREATE INDEX acc_lines_account_idx ON acc_lines (account_id ASC, date ASC);
CREATE INDEX acc_lines_party_idx ON acc_lines (party_id ASC) WHERE party_id IS NOT NULL;

-- CRM invoices in the books.
CREATE TABLE acc_invoices (
    item_id    UUID PRIMARY KEY REFERENCES crm_items (id),
    entry_id   UUID NOT NULL REFERENCES acc_entries (id),
    date       DATE NOT NULL,
    due_date   DATE,
    party_id   UUID REFERENCES crm_items (id) ON DELETE SET NULL,
    currency   CHAR(3) NOT NULL,
    -- Units of the base currency per unit of the invoice's.
    fx_rate    NUMERIC(24, 10) NOT NULL,
    total      NUMERIC(20, 2) NOT NULL,
    -- open | paid | void
    status     TEXT NOT NULL DEFAULT 'open' CHECK (status IN ('open', 'paid', 'void')),
    posted_by  UUID REFERENCES users (id) ON DELETE SET NULL,
    posted_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Bills from suppliers.
CREATE TABLE acc_bills (
    id             UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    number         BIGINT UNIQUE,
    -- The supplier's own invoice number.
    reference      TEXT,
    party_id       UUID REFERENCES crm_items (id) ON DELETE SET NULL,
    date           DATE NOT NULL,
    due_date       DATE,
    currency       CHAR(3) NOT NULL,
    fx_rate        NUMERIC(24, 10) NOT NULL DEFAULT 1,
    -- Prices on the lines include tax.
    tax_inclusive  BOOLEAN NOT NULL DEFAULT FALSE,
    subtotal       NUMERIC(20, 2) NOT NULL DEFAULT 0,
    tax            NUMERIC(20, 2) NOT NULL DEFAULT 0,
    total          NUMERIC(20, 2) NOT NULL DEFAULT 0,
    -- draft | open | paid | void
    status         TEXT NOT NULL DEFAULT 'draft' CHECK (status IN ('draft', 'open', 'paid', 'void')),
    entry_id       UUID REFERENCES acc_entries (id),
    memo           TEXT,
    file_ids       UUID[] NOT NULL DEFAULT '{}',
    created_by     UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX acc_bills_party_idx ON acc_bills (party_id ASC);
CREATE INDEX acc_bills_status_idx ON acc_bills (status, due_date);

CREATE TABLE acc_bill_lines (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    bill_id     UUID NOT NULL REFERENCES acc_bills (id) ON DELETE CASCADE,
    description TEXT NOT NULL,
    account_id  UUID NOT NULL REFERENCES acc_accounts (id),
    quantity    NUMERIC(18, 4) NOT NULL DEFAULT 1,
    unit_price  NUMERIC(20, 4) NOT NULL,
    tax_id      UUID REFERENCES acc_taxes (id),
    net         NUMERIC(20, 2) NOT NULL,
    tax         NUMERIC(20, 2) NOT NULL DEFAULT 0,
    sort        INT NOT NULL DEFAULT 0
);
CREATE INDEX acc_bill_lines_bill_idx ON acc_bill_lines (bill_id ASC, sort ASC);

-- Money received (in) or paid (out) through a bank or cash account.
CREATE TABLE acc_payments (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    number      BIGINT NOT NULL UNIQUE,
    direction   TEXT NOT NULL CHECK (direction IN ('in', 'out')),
    date        DATE NOT NULL,
    account_id  UUID NOT NULL REFERENCES acc_accounts (id),
    party_id    UUID REFERENCES crm_items (id) ON DELETE SET NULL,
    currency    CHAR(3) NOT NULL,
    amount      NUMERIC(20, 2) NOT NULL CHECK (amount > 0),
    fx_rate     NUMERIC(24, 10) NOT NULL DEFAULT 1,
    reference   TEXT,
    memo        TEXT,
    -- posted | void
    status      TEXT NOT NULL DEFAULT 'posted' CHECK (status IN ('posted', 'void')),
    entry_id    UUID REFERENCES acc_entries (id),
    created_by  UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX acc_payments_party_idx ON acc_payments (party_id ASC);

-- Which invoices and bills a payment settles (amounts in the document's currency).
CREATE TABLE acc_allocations (
    payment_id UUID NOT NULL REFERENCES acc_payments (id) ON DELETE CASCADE,
    -- invoice: a CRM invoice (acc_invoices.item_id); bill: acc_bills.id
    doc_kind   TEXT NOT NULL CHECK (doc_kind IN ('invoice', 'bill')),
    doc_id     UUID NOT NULL,
    amount     NUMERIC(20, 2) NOT NULL CHECK (amount > 0),
    PRIMARY KEY (payment_id, doc_kind, doc_id)
);
CREATE INDEX acc_allocations_doc_idx ON acc_allocations (doc_kind, doc_id);

-- Bank statement lines, imported and matched to the books.
CREATE TABLE acc_bank_lines (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    account_id  UUID NOT NULL REFERENCES acc_accounts (id),
    date        DATE NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    reference   TEXT,
    -- Money in positive, out negative, in the account's currency.
    amount      NUMERIC(20, 2) NOT NULL,
    -- unmatched | matched | ignored
    status      TEXT NOT NULL DEFAULT 'unmatched' CHECK (status IN ('unmatched', 'matched', 'ignored')),
    -- The journal line on the bank account it corresponds to.
    line_id     UUID UNIQUE REFERENCES acc_lines (id) ON DELETE SET NULL,
    -- Same statement line imported twice is skipped.
    fingerprint TEXT NOT NULL,
    imported_by UUID REFERENCES users (id) ON DELETE SET NULL,
    imported_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (account_id, fingerprint)
);
CREATE INDEX acc_bank_lines_account_idx ON acc_bank_lines (account_id ASC, date DESC);

-- Income and expense accounts per product (otherwise the default sales account).
CREATE TABLE acc_product_accounts (
    product_id        UUID PRIMARY KEY REFERENCES crm_products (id) ON DELETE CASCADE,
    income_account_id UUID REFERENCES acc_accounts (id)
);
