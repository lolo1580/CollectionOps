-- Item lifecycle. An item is `active`, `archived` or in the trash (`trashed`). Every state is
-- reversible, so no physical deletion path exists until the retention rules are decided.
ALTER TABLE inventory_items
    ADD COLUMN state VARCHAR(16) CHARACTER SET ascii COLLATE ascii_bin NOT NULL DEFAULT 'active';

ALTER TABLE inventory_items
    ADD CONSTRAINT chk_inventory_items_state CHECK (state IN ('active', 'archived', 'trashed'));

-- Every transition records the actor and the exact before/after state, like membership changes.
CREATE TABLE inventory_item_audit_events (
    id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    item_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    space_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    actor_account_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    event_code VARCHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    state_before VARCHAR(16) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    state_after VARCHAR(16) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    created_at DATETIME(6) NOT NULL,
    PRIMARY KEY (id),
    KEY idx_inventory_item_audit_item_date (item_id, created_at),
    KEY idx_inventory_item_audit_space_date (space_id, created_at),
    CONSTRAINT fk_inventory_item_audit_item FOREIGN KEY (item_id) REFERENCES inventory_items (id),
    CONSTRAINT fk_inventory_item_audit_space FOREIGN KEY (space_id) REFERENCES spaces (id),
    CONSTRAINT fk_inventory_item_audit_actor FOREIGN KEY (actor_account_id) REFERENCES accounts (id),
    CONSTRAINT chk_inventory_item_audit_event CHECK (event_code IN ('archived', 'trashed', 'restored')),
    CONSTRAINT chk_inventory_item_audit_states CHECK (
        state_before IN ('active', 'archived', 'trashed')
        AND state_after IN ('active', 'archived', 'trashed')
    )
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
