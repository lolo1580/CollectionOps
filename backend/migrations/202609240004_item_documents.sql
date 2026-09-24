-- Documents belong to the space where they were uploaded, even after an item transfer.
-- Files are stored outside MariaDB under an operator-controlled directory.
CREATE TABLE item_documents (
    id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    item_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    space_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    original_name VARCHAR(255) NOT NULL,
    media_type VARCHAR(32) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    byte_size BIGINT UNSIGNED NOT NULL,
    sha256 CHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    uploaded_by_account_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    created_at DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    deleted_at DATETIME(6) NULL,
    PRIMARY KEY (id),
    KEY idx_item_documents_item_space (item_id, space_id, deleted_at),
    CONSTRAINT fk_item_documents_item FOREIGN KEY (item_id) REFERENCES inventory_items (id),
    CONSTRAINT fk_item_documents_space FOREIGN KEY (space_id) REFERENCES spaces (id),
    CONSTRAINT fk_item_documents_account FOREIGN KEY (uploaded_by_account_id) REFERENCES accounts (id),
    CONSTRAINT chk_item_documents_size CHECK (byte_size > 0 AND byte_size <= 50000000),
    CONSTRAINT chk_item_documents_type CHECK (media_type IN ('image/jpeg', 'image/png', 'image/webp', 'application/pdf'))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

-- Existing owners could not grant these rights to themselves through the API. Give the owner
-- explicit document grants, just as new spaces do; finance remains separate and ungranted.
INSERT IGNORE INTO space_permission_grants (space_id, account_id, permission_code)
SELECT s.id, s.owner_account_id, 'documents_read' FROM spaces s;
INSERT IGNORE INTO space_permission_grants (space_id, account_id, permission_code)
SELECT s.id, s.owner_account_id, 'documents_write' FROM spaces s;
