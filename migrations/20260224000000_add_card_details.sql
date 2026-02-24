-- Add new fields for CSV export requirements
ALTER TABLE cards ADD COLUMN rarity TEXT DEFAULT 'Unknown';
ALTER TABLE cards ADD COLUMN card_number INTEGER DEFAULT 0;
