-- Persist the actor, target and exact grant transition for membership administration.
CREATE TABLE space_member_audit_events (
    id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    space_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    actor_account_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    target_account_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    event_code VARCHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    grants_before VARCHAR(512) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    grants_after VARCHAR(512) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    created_at DATETIME(6) NOT NULL,
    PRIMARY KEY (id),
    KEY idx_member_audit_space_date (space_id, created_at),
    CONSTRAINT fk_member_audit_space FOREIGN KEY (space_id) REFERENCES spaces (id),
    CONSTRAINT fk_member_audit_actor FOREIGN KEY (actor_account_id) REFERENCES accounts (id),
    CONSTRAINT fk_member_audit_target FOREIGN KEY (target_account_id) REFERENCES accounts (id),
    CONSTRAINT chk_member_audit_event CHECK (event_code IN ('permissions_changed', 'member_removed'))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
