-- A space manager is a member to whom the owner delegates member administration. The composite
-- foreign key ties a manager to an existing membership, so removing the member also removes the
-- delegation. Being a manager never grants collection or financial rights, only the ability to
-- manage other members (never the owner and never other managers).
CREATE TABLE space_managers (
    space_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    account_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    appointed_by_account_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    created_at DATETIME(6) NOT NULL,
    PRIMARY KEY (space_id, account_id),
    KEY idx_space_managers_account (account_id),
    CONSTRAINT fk_space_managers_membership FOREIGN KEY (space_id, account_id)
        REFERENCES space_memberships (space_id, account_id) ON DELETE CASCADE,
    CONSTRAINT fk_space_managers_actor FOREIGN KEY (appointed_by_account_id) REFERENCES accounts (id)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE space_manager_events (
    id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    space_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    actor_account_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    target_account_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    event_code VARCHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    created_at DATETIME(6) NOT NULL,
    PRIMARY KEY (id),
    KEY idx_space_manager_events_space_date (space_id, created_at),
    CONSTRAINT fk_space_manager_events_space FOREIGN KEY (space_id) REFERENCES spaces (id),
    CONSTRAINT fk_space_manager_events_actor FOREIGN KEY (actor_account_id) REFERENCES accounts (id),
    CONSTRAINT fk_space_manager_events_target FOREIGN KEY (target_account_id) REFERENCES accounts (id),
    CONSTRAINT chk_space_manager_events_code CHECK (event_code IN ('manager_appointed', 'manager_revoked'))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
