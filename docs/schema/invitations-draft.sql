-- Brouillon à transformer en migration après définition de l'identité e-mail,
-- du parcours d'acceptation et de l'audit. Nécessite la migration du socle.
-- Le jeton livré par e-mail n'est jamais conservé en clair.

CREATE TABLE space_invitations (
    id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    space_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    recipient_email VARCHAR(320) NOT NULL,
    token_fingerprint BINARY(32) NOT NULL,
    inviter_account_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    accepted_account_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NULL,
    created_at DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    expires_at DATETIME(6) NOT NULL,
    accepted_at DATETIME(6) NULL,
    revoked_at DATETIME(6) NULL,
    PRIMARY KEY (id),
    UNIQUE KEY uq_space_invitations_token_fingerprint (token_fingerprint),
    KEY idx_space_invitations_space_email (space_id, recipient_email),
    KEY idx_space_invitations_inviter (inviter_account_id),
    KEY idx_space_invitations_accepted (accepted_account_id),
    CONSTRAINT fk_space_invitations_space FOREIGN KEY (space_id) REFERENCES spaces (id),
    CONSTRAINT fk_space_invitations_inviter FOREIGN KEY (inviter_account_id) REFERENCES accounts (id),
    CONSTRAINT fk_space_invitations_accepted FOREIGN KEY (accepted_account_id) REFERENCES accounts (id),
    CONSTRAINT chk_space_invitations_email CHECK (CHAR_LENGTH(TRIM(recipient_email)) > 0),
    CONSTRAINT chk_space_invitations_expiry CHECK (expires_at > created_at),
    CONSTRAINT chk_space_invitations_acceptance CHECK (
        (accepted_at IS NULL AND accepted_account_id IS NULL)
        OR (accepted_at IS NOT NULL AND accepted_account_id IS NOT NULL)
    ),
    CONSTRAINT chk_space_invitations_terminal CHECK (accepted_at IS NULL OR revoked_at IS NULL)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

-- La création d'une invitation insère collections_read par défaut ; le
-- propriétaire peut choisir d'autres droits d'espace avant l'envoi.
CREATE TABLE space_invitation_grants (
    invitation_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    permission_code VARCHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    PRIMARY KEY (invitation_id, permission_code),
    CONSTRAINT fk_space_invitation_grants_invitation FOREIGN KEY (invitation_id)
        REFERENCES space_invitations (id),
    CONSTRAINT chk_invitation_grants_permission CHECK (permission_code IN (
        'collections_read', 'collections_write',
        'acquisitions_read', 'acquisitions_write',
        'finance_read', 'finance_write',
        'documents_read', 'documents_write'
    ))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
