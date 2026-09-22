-- Per-device sessions. One row per client, never shared between devices.
-- Only the SHA-256 fingerprint of the token is stored: a database leak yields no
-- reusable credential. The plaintext token exists only in the login response.

CREATE TABLE account_sessions (
    id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    account_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    token_fingerprint BINARY(32) NOT NULL,
    device_label VARCHAR(255) NULL,
    idle_timeout_seconds INT UNSIGNED NOT NULL,
    created_at DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    last_seen_at DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    absolute_expires_at DATETIME(6) NOT NULL,
    revoked_at DATETIME(6) NULL,
    PRIMARY KEY (id),
    UNIQUE KEY uq_account_sessions_token_fingerprint (token_fingerprint),
    KEY idx_account_sessions_account (account_id),
    KEY idx_account_sessions_expiry (absolute_expires_at),
    CONSTRAINT fk_account_sessions_account FOREIGN KEY (account_id) REFERENCES accounts (id),
    -- The seven-day ceiling is a server rule, not a client preference: the menu may only
    -- choose a shorter idle delay, never a longer token lifetime.
    CONSTRAINT chk_account_sessions_idle_timeout CHECK (
        idle_timeout_seconds BETWEEN 60 AND 604800
    ),
    CONSTRAINT chk_account_sessions_absolute_expiry CHECK (
        absolute_expires_at > created_at
        AND absolute_expires_at <= DATE_ADD(created_at, INTERVAL 7 DAY)
    ),
    CONSTRAINT chk_account_sessions_last_seen CHECK (
        last_seen_at >= created_at AND last_seen_at <= absolute_expires_at
    )
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
