-- Initial non-financial acquisition pipeline. Prices and valuations are deliberately absent.
INSERT IGNORE INTO space_permission_grants (space_id, account_id, permission_code)
SELECT s.id, s.owner_account_id, 'acquisitions_read'
FROM spaces s JOIN space_memberships m
  ON m.space_id = s.id AND m.account_id = s.owner_account_id;

INSERT IGNORE INTO space_permission_grants (space_id, account_id, permission_code)
SELECT s.id, s.owner_account_id, 'acquisitions_write'
FROM spaces s JOIN space_memberships m
  ON m.space_id = s.id AND m.account_id = s.owner_account_id;

CREATE TABLE wishlist_entries (
    id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    space_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    title VARCHAR(255) NOT NULL,
    search_notes TEXT NULL,
    created_at DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    PRIMARY KEY (id),
    UNIQUE KEY uq_wishlist_space_id (space_id, id),
    KEY idx_wishlist_space_created (space_id, created_at),
    CONSTRAINT fk_wishlist_space FOREIGN KEY (space_id) REFERENCES spaces (id),
    CONSTRAINT chk_wishlist_title CHECK (CHAR_LENGTH(TRIM(title)) > 0)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE acquisition_vendors (
    id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    space_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    name VARCHAR(255) NOT NULL,
    website_url VARCHAR(2048) NULL,
    created_at DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    PRIMARY KEY (id),
    UNIQUE KEY uq_vendors_space_id (space_id, id),
    UNIQUE KEY uq_vendors_space_name (space_id, name),
    CONSTRAINT fk_vendors_space FOREIGN KEY (space_id) REFERENCES spaces (id),
    CONSTRAINT chk_vendors_name CHECK (CHAR_LENGTH(TRIM(name)) > 0)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE acquisition_offers (
    id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    space_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    wish_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    vendor_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    title VARCHAR(255) NOT NULL,
    source_url VARCHAR(2048) NULL,
    notes TEXT NULL,
    created_at DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    PRIMARY KEY (id),
    KEY idx_offers_wish (space_id, wish_id, created_at),
    KEY idx_offers_vendor (space_id, vendor_id),
    CONSTRAINT fk_offers_wish FOREIGN KEY (space_id, wish_id)
        REFERENCES wishlist_entries (space_id, id),
    CONSTRAINT fk_offers_vendor FOREIGN KEY (space_id, vendor_id)
        REFERENCES acquisition_vendors (space_id, id),
    CONSTRAINT chk_offers_title CHECK (CHAR_LENGTH(TRIM(title)) > 0)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
