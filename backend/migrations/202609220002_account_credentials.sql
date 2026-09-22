-- Local credentials for accounts, created before any session or login route.
-- Only the Argon2id PHC hash is stored; plaintext passwords never reach MariaDB.
-- OIDC or other identity providers can coexist later through a distinct table.

ALTER TABLE accounts
    ADD COLUMN email VARCHAR(320) CHARACTER SET ascii COLLATE ascii_general_ci NULL,
    ADD COLUMN password_hash VARCHAR(255) CHARACTER SET ascii COLLATE ascii_bin NULL,
    ADD COLUMN email_verified_at DATETIME(6) NULL,
    ADD CONSTRAINT chk_accounts_email CHECK (
        email IS NULL OR CHAR_LENGTH(TRIM(email)) > 0
    );

-- Unique per non-null address: MariaDB allows several NULL rows in a unique key.
CREATE UNIQUE INDEX uq_accounts_email ON accounts (email);
