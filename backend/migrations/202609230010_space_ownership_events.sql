-- Ownership transfer is a distinct, audited operation. The new owner must already be a
-- member, and the previous owner keeps its membership and grants: ownership never implies
-- collection or financial rights. The check forbids a no-op transfer to the current owner.
CREATE TABLE space_ownership_events (
    id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    space_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    previous_owner_account_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    new_owner_account_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    actor_account_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    created_at DATETIME(6) NOT NULL,
    PRIMARY KEY (id),
    KEY idx_space_ownership_events_space_date (space_id, created_at),
    CONSTRAINT fk_space_ownership_events_space FOREIGN KEY (space_id) REFERENCES spaces (id),
    CONSTRAINT fk_space_ownership_events_previous FOREIGN KEY (previous_owner_account_id) REFERENCES accounts (id),
    CONSTRAINT fk_space_ownership_events_new FOREIGN KEY (new_owner_account_id) REFERENCES accounts (id),
    CONSTRAINT fk_space_ownership_events_actor FOREIGN KEY (actor_account_id) REFERENCES accounts (id),
    CONSTRAINT chk_space_ownership_events_distinct CHECK (
        previous_owner_account_id <> new_owner_account_id
    )
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
