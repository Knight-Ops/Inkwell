CREATE TABLE IF NOT EXISTS system_stats (
    key TEXT PRIMARY KEY,
    value INTEGER DEFAULT 0
);

INSERT OR IGNORE INTO system_stats (key, value) VALUES ('total_scanned_cards', 0);
