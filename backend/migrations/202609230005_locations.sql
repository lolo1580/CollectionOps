-- Space-scoped physical locations form a tree. The non-null parent_key enforces
-- root-name uniqueness in MariaDB, where UNIQUE permits duplicate NULL values.
CREATE TABLE inventory_locations (
    id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    space_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    parent_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NULL,
    parent_key CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    name VARCHAR(255) NOT NULL,
    created_at DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    PRIMARY KEY (id),
    UNIQUE KEY uq_inventory_locations_space_id (space_id, id),
    UNIQUE KEY uq_inventory_locations_sibling_name (space_id, parent_key, name),
    KEY idx_inventory_locations_parent (space_id, parent_id),
    CONSTRAINT fk_inventory_locations_space FOREIGN KEY (space_id) REFERENCES spaces (id),
    CONSTRAINT fk_inventory_locations_parent FOREIGN KEY (space_id, parent_id)
        REFERENCES inventory_locations (space_id, id),
    CONSTRAINT chk_inventory_locations_name CHECK (CHAR_LENGTH(TRIM(name)) > 0),
    CONSTRAINT chk_inventory_locations_parent_key CHECK (
        (parent_id IS NULL AND parent_key = '00000000-0000-0000-0000-000000000000')
        OR (parent_id IS NOT NULL AND parent_key = parent_id)
    )
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

-- One nullable location on an item means zero or one current physical position.
-- The composite FK also prevents assigning a location from another space.
ALTER TABLE inventory_items
    ADD COLUMN current_location_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NULL,
    ADD KEY idx_inventory_items_current_location (space_id, current_location_id),
    ADD CONSTRAINT fk_inventory_items_current_location
        FOREIGN KEY (space_id, current_location_id) REFERENCES inventory_locations (space_id, id);

CREATE TABLE item_location_events (
    id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    item_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    space_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    from_location_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NULL,
    to_location_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NULL,
    actor_account_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    created_at DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    PRIMARY KEY (id),
    KEY idx_item_location_events_item_space (item_id, space_id, created_at),
    KEY idx_item_location_events_from (space_id, from_location_id),
    KEY idx_item_location_events_to (space_id, to_location_id),
    CONSTRAINT fk_item_location_events_item FOREIGN KEY (item_id) REFERENCES inventory_items (id),
    CONSTRAINT fk_item_location_events_space FOREIGN KEY (space_id) REFERENCES spaces (id),
    CONSTRAINT fk_item_location_events_from FOREIGN KEY (space_id, from_location_id)
        REFERENCES inventory_locations (space_id, id),
    CONSTRAINT fk_item_location_events_to FOREIGN KEY (space_id, to_location_id)
        REFERENCES inventory_locations (space_id, id),
    CONSTRAINT fk_item_location_events_actor FOREIGN KEY (actor_account_id) REFERENCES accounts (id),
    CONSTRAINT chk_item_location_events_changed CHECK (
        NOT (from_location_id <=> to_location_id)
    )
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
