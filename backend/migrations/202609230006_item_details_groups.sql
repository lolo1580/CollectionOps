ALTER TABLE inventory_items
    ADD COLUMN description TEXT NULL AFTER name,
    ADD COLUMN historical_reference VARCHAR(500) NULL AFTER description,
    ADD COLUMN technical_reference VARCHAR(500) NULL AFTER historical_reference,
    ADD UNIQUE KEY uq_inventory_items_space_id (space_id, id);

CREATE TABLE inventory_groups (
    id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    space_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    kind VARCHAR(16) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    name VARCHAR(255) NOT NULL,
    created_at DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    PRIMARY KEY (id),
    UNIQUE KEY uq_inventory_groups_space_kind_name (space_id, kind, name),
    UNIQUE KEY uq_inventory_groups_space_id (space_id, id),
    CONSTRAINT fk_inventory_groups_space FOREIGN KEY (space_id) REFERENCES spaces (id),
    CONSTRAINT chk_inventory_groups_kind CHECK (kind IN ('series', 'group')),
    CONSTRAINT chk_inventory_groups_name CHECK (CHAR_LENGTH(TRIM(name)) > 0)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE inventory_group_members (
    space_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    group_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    item_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    PRIMARY KEY (group_id, item_id),
    KEY idx_inventory_group_members_item (item_id),
    CONSTRAINT fk_inventory_group_members_group FOREIGN KEY (space_id, group_id)
        REFERENCES inventory_groups (space_id, id),
    CONSTRAINT fk_inventory_group_members_item FOREIGN KEY (space_id, item_id)
        REFERENCES inventory_items (space_id, id)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
