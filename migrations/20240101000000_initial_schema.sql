-- Idempotency for dev (allows migration to run even if tables exist from build setup)
DROP TABLE IF EXISTS collection;
DROP TABLE IF EXISTS cards;

-- 1. Knowledge Base (Read-Only during operation)
CREATE TABLE cards (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    subtitle TEXT NOT NULL,
    -- set_code is useful for display and filtering
    set_code TEXT NOT NULL,
    image_url TEXT NOT NULL,
    phash TEXT NOT NULL,
    meta_json TEXT
);

-- Index for potential future optimization on phash lookups (though we use in-memory mostly)
CREATE INDEX idx_cards_phash ON cards(phash);

-- 2. User Inventory (Read/Write)
CREATE TABLE collection (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    card_id TEXT NOT NULL,
    quantity INTEGER DEFAULT 1,
    is_foil BOOLEAN DEFAULT 0,
    added_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY(card_id) REFERENCES cards(id)
);
