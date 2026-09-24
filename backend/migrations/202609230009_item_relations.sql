-- Directed, typed links an item declares to another item of the same space. The composite
-- foreign keys on (space_id, item_id) make a cross-space link impossible, and the check
-- forbids an item from linking to itself. Links are removed when one endpoint is transferred.
CREATE TABLE item_relations (
    id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    space_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    source_item_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    target_item_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    kind VARCHAR(16) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    created_by_account_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    created_at DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    PRIMARY KEY (id),
    UNIQUE KEY uq_item_relations_edge (source_item_id, target_item_id, kind),
    KEY idx_item_relations_source (space_id, source_item_id),
    KEY idx_item_relations_target (space_id, target_item_id),
    KEY idx_item_relations_creator (created_by_account_id),
    CONSTRAINT fk_item_relations_space FOREIGN KEY (space_id) REFERENCES spaces (id),
    CONSTRAINT fk_item_relations_source FOREIGN KEY (space_id, source_item_id)
        REFERENCES inventory_items (space_id, id),
    CONSTRAINT fk_item_relations_target FOREIGN KEY (space_id, target_item_id)
        REFERENCES inventory_items (space_id, id),
    CONSTRAINT fk_item_relations_creator FOREIGN KEY (created_by_account_id) REFERENCES accounts (id),
    CONSTRAINT chk_item_relations_kind CHECK (kind IN ('related', 'variant_of', 'part_of')),
    CONSTRAINT chk_item_relations_distinct CHECK (source_item_id <> target_item_id)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
