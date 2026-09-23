-- Invitation secrets are stored only as SHA-256 fingerprints. An invitation is single-use.
CREATE TABLE space_invitations (
    id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    space_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    recipient_email VARCHAR(320) CHARACTER SET ascii COLLATE ascii_general_ci NOT NULL,
    token_fingerprint BINARY(32) NOT NULL,
    inviter_account_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    accepted_account_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NULL,
    created_at DATETIME(6) NOT NULL,
    expires_at DATETIME(6) NOT NULL,
    accepted_at DATETIME(6) NULL,
    revoked_at DATETIME(6) NULL,
    PRIMARY KEY (id),
    UNIQUE KEY uq_space_invitations_fingerprint (token_fingerprint),
    KEY idx_space_invitations_space_email (space_id, recipient_email),
    CONSTRAINT fk_space_invitations_space FOREIGN KEY (space_id) REFERENCES spaces (id),
    CONSTRAINT fk_space_invitations_inviter FOREIGN KEY (inviter_account_id) REFERENCES accounts (id),
    CONSTRAINT fk_space_invitations_acceptor FOREIGN KEY (accepted_account_id) REFERENCES accounts (id),
    CONSTRAINT chk_space_invitations_expiry CHECK (expires_at > created_at)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE space_invitation_grants (
    invitation_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    permission_code VARCHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    PRIMARY KEY (invitation_id, permission_code),
    CONSTRAINT fk_space_invitation_grants_invitation FOREIGN KEY (invitation_id)
        REFERENCES space_invitations (id),
    CONSTRAINT chk_space_invitation_grants_permission CHECK (permission_code IN (
        'collections_read', 'collections_write',
        'acquisitions_read', 'acquisitions_write',
        'finance_read', 'finance_write',
        'documents_read', 'documents_write'
    ))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE space_audit_events (
    id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    space_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    actor_account_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    invitation_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NULL,
    event_code VARCHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    created_at DATETIME(6) NOT NULL,
    PRIMARY KEY (id),
    KEY idx_space_audit_events_space_date (space_id, created_at),
    CONSTRAINT fk_space_audit_events_space FOREIGN KEY (space_id) REFERENCES spaces (id),
    CONSTRAINT fk_space_audit_events_actor FOREIGN KEY (actor_account_id) REFERENCES accounts (id),
    CONSTRAINT fk_space_audit_events_invitation FOREIGN KEY (invitation_id) REFERENCES space_invitations (id),
    CONSTRAINT chk_space_audit_events_code CHECK (event_code IN (
        'invitation_created', 'invitation_revoked', 'invitation_accepted'
    ))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
