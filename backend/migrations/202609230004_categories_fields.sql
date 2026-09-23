-- Categories are scoped to a space. A sentinel parent key makes root-name uniqueness real:
-- UNIQUE on a nullable parent_id alone would permit duplicate root names in MariaDB.
CREATE TABLE inventory_categories (
    id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    space_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    parent_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NULL,
    parent_key CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    name VARCHAR(255) NOT NULL,
    created_at DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    PRIMARY KEY (id),
    UNIQUE KEY uq_inventory_categories_space_id (space_id, id),
    UNIQUE KEY uq_inventory_categories_sibling_name (space_id, parent_key, name),
    KEY idx_inventory_categories_parent (space_id, parent_id),
    CONSTRAINT fk_inventory_categories_space FOREIGN KEY (space_id) REFERENCES spaces (id),
    CONSTRAINT fk_inventory_categories_parent FOREIGN KEY (space_id, parent_id)
        REFERENCES inventory_categories (space_id, id),
    CONSTRAINT chk_inventory_categories_name CHECK (CHAR_LENGTH(TRIM(name)) > 0),
    CONSTRAINT chk_inventory_categories_parent_key CHECK (
        (parent_id IS NULL AND parent_key = '00000000-0000-0000-0000-000000000000')
        OR (parent_id IS NOT NULL AND parent_key = parent_id)
    )
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

-- A field belongs to one category. Its value type is fixed when it is created.
CREATE TABLE category_field_definitions (
    id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    category_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    name VARCHAR(255) NOT NULL,
    value_type VARCHAR(16) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    created_at DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    PRIMARY KEY (id),
    UNIQUE KEY uq_category_field_name (category_id, name),
    CONSTRAINT fk_category_field_category FOREIGN KEY (category_id) REFERENCES inventory_categories (id),
    CONSTRAINT chk_category_field_name CHECK (CHAR_LENGTH(TRIM(name)) > 0),
    CONSTRAINT chk_category_field_type CHECK (value_type IN ('text', 'number', 'date'))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

-- Ending an assignment keeps category-specific values for historical inspection without
-- displaying an old space's taxonomy in the item's current space.
CREATE TABLE item_category_assignments (
    id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    item_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    category_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    space_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    assigned_at DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    ended_at DATETIME(6) NULL,
    PRIMARY KEY (id),
    KEY idx_item_category_active (item_id, space_id, ended_at),
    KEY idx_item_category_category (category_id),
    CONSTRAINT fk_item_category_item FOREIGN KEY (item_id) REFERENCES inventory_items (id),
    CONSTRAINT fk_item_category_category FOREIGN KEY (space_id, category_id)
        REFERENCES inventory_categories (space_id, id)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE item_custom_field_values (
    assignment_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    field_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    value_text TEXT NULL,
    value_number DECIMAL(20,6) NULL,
    value_date DATE NULL,
    updated_at DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    PRIMARY KEY (assignment_id, field_id),
    KEY idx_item_custom_field_definition (field_id),
    CONSTRAINT fk_item_field_assignment FOREIGN KEY (assignment_id) REFERENCES item_category_assignments (id),
    CONSTRAINT fk_item_field_definition FOREIGN KEY (field_id) REFERENCES category_field_definitions (id),
    CONSTRAINT chk_item_field_one_value CHECK (
        (value_text IS NOT NULL) + (value_number IS NOT NULL) + (value_date IS NOT NULL) = 1
    )
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
