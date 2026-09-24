-- A client may retry one creation after a lost response without reserving another number.
-- Keep the original response snapshot: the item may later be edited or transferred.
CREATE TABLE item_creation_requests (
    space_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    account_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    request_key CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    requested_name VARCHAR(255) NOT NULL,
    item_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NULL,
    inventory_number BIGINT UNSIGNED NULL,
    item_created_at DATETIME(6) NULL,
    created_at DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    PRIMARY KEY (space_id, account_id, request_key),
    KEY idx_item_creation_requests_item (item_id),
    CONSTRAINT fk_item_creation_requests_space FOREIGN KEY (space_id) REFERENCES spaces (id),
    CONSTRAINT fk_item_creation_requests_account FOREIGN KEY (account_id) REFERENCES accounts (id),
    CONSTRAINT fk_item_creation_requests_item FOREIGN KEY (item_id) REFERENCES inventory_items (id),
    CONSTRAINT chk_item_creation_requests_snapshot CHECK (
        (item_id IS NULL AND inventory_number IS NULL AND item_created_at IS NULL) OR
        (item_id IS NOT NULL AND inventory_number IS NOT NULL AND item_created_at IS NOT NULL)
    )
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
