-- Preserve existing prospects and start optimistic revisions at one.
ALTER TABLE wishlist_entries ADD COLUMN revision BIGINT UNSIGNED NOT NULL DEFAULT 1;
ALTER TABLE acquisition_vendors ADD COLUMN revision BIGINT UNSIGNED NOT NULL DEFAULT 1;
ALTER TABLE acquisition_offers ADD COLUMN revision BIGINT UNSIGNED NOT NULL DEFAULT 1;
