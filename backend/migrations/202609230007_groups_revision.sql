ALTER TABLE inventory_groups
    ADD COLUMN revision BIGINT UNSIGNED NOT NULL DEFAULT 1 AFTER name,
    ADD CONSTRAINT chk_inventory_groups_revision CHECK (revision > 0);
