-- Kindle support: account connection, target devices, and delivery tracking.

CREATE TABLE kindle_accounts (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) UNIQUE,
    credentials_json TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE kindle_targets (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    kindle_account_id UUID NOT NULL REFERENCES kindle_accounts(id) ON DELETE CASCADE,
    device_id UUID REFERENCES devices(id) ON DELETE SET NULL,
    device_serial TEXT NOT NULL,
    device_name TEXT,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(kindle_account_id, device_serial)
);

CREATE TABLE kindle_deliveries (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    kindle_target_id UUID NOT NULL REFERENCES kindle_targets(id) ON DELETE CASCADE,
    book_id UUID NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    book_file_id UUID NOT NULL REFERENCES book_files(id) ON DELETE CASCADE,
    delivered_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(kindle_target_id, book_id)
);

CREATE INDEX idx_kindle_deliveries_book ON kindle_deliveries(book_id);
