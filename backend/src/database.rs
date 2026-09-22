use std::{error::Error, fmt};

use chrono::{DateTime, Duration, Utc};
use sqlx::{MySqlPool, Row, mysql::MySqlPoolOptions};
use uuid::Uuid;

use crate::{
    config::BootstrapAdmin,
    credentials::{
        EmailAddress, IdleTimeout, PasswordHashValue, PasswordService, SessionLifetime,
        SessionToken,
    },
};

/// A session row as shown to its owner, without any secret material.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRecord {
    pub id: String,
    pub device_label: Option<String>,
    pub idle_timeout: IdleTimeout,
    pub created_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    pub absolute_expires_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
}

/// A freshly issued session. The bearer token is returned exactly once, at login.
pub struct IssuedSession {
    pub session: SessionRecord,
    token: SessionToken,
}

impl IssuedSession {
    /// Exposes the bearer token to the caller that must hand it to the client.
    /// It is never stored in plaintext and never logged.
    #[must_use]
    pub fn token(&self) -> &SessionToken {
        &self.token
    }
}

/// Why a presented token was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionError {
    /// No session matches this token.
    Unknown,
    /// The session exists but may no longer be used: revoked, idle for too long, or past its
    /// absolute expiry.
    Expired,
    /// The account the session belonged to no longer exists.
    AccountMissing,
    /// The storage layer failed.
    Storage,
}

/// A connected MariaDB pool with all embedded migrations applied.
#[derive(Clone)]
pub struct Database {
    pool: MySqlPool,
}

impl Database {
    /// Connects to MariaDB and applies the versioned backend migrations.
    /// The caller must provide a URL whose TLS settings match the deployment policy.
    ///
    /// # Errors
    ///
    /// Returns [`DatabaseError`] if the connection or a migration fails.
    pub async fn connect_and_migrate(database_url: &str) -> Result<Self, DatabaseError> {
        let pool = MySqlPoolOptions::new()
            .max_connections(10)
            .connect(database_url)
            .await
            .map_err(DatabaseError::Connection)?;

        if let Err(error) = sqlx::migrate!("./migrations").run(&pool).await {
            pool.close().await;
            return Err(DatabaseError::Migration(error));
        }

        Ok(Self { pool })
    }

    #[must_use]
    pub const fn pool(&self) -> &MySqlPool {
        &self.pool
    }

    /// Creates the first administrator when the installation has none.
    ///
    /// Requirement D01 allows exactly one installation-time administrator and no free sign-up
    /// route. Provisioning is therefore idempotent: if any account already carries the system
    /// administrator flag, the call leaves the database untouched and reports `false`.
    /// The password is hashed with Argon2id before it reaches the database.
    ///
    /// # Errors
    ///
    /// Returns [`DatabaseError`] when the administrator lookup, the password hashing, or the
    /// insert fails.
    pub async fn provision_bootstrap_admin(
        &self,
        admin: &BootstrapAdmin,
        passwords: &PasswordService,
    ) -> Result<bool, DatabaseError> {
        let mut transaction = self.pool.begin().await.map_err(DatabaseError::Query)?;

        // `accounts.id` is CHAR(36) in an ascii_bin collation, which MySQL reports as BINARY.
        // Counting keeps the sentinel value an integer, and no extra lock is needed: the
        // unique index on `email` still protects against a concurrent double provisioning.
        let existing: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM accounts WHERE is_system_admin = TRUE")
                .fetch_one(&mut *transaction)
                .await
                .map_err(DatabaseError::Query)?;

        if existing > 0 {
            transaction.commit().await.map_err(DatabaseError::Query)?;
            return Ok(false);
        }

        let hash = passwords
            .hash_password(admin.password())
            .map_err(DatabaseError::Password)?;
        let id = Uuid::now_v7().to_string();

        sqlx::query(
            "INSERT INTO accounts \
                 (id, display_name, is_system_admin, email, password_hash, email_verified_at) \
             VALUES (?, ?, TRUE, ?, ?, CURRENT_TIMESTAMP(6))",
        )
        .bind(&id)
        .bind(admin.display_name())
        .bind(admin.email().as_str())
        .bind(hash.as_str())
        .execute(&mut *transaction)
        .await
        .map_err(DatabaseError::Query)?;

        transaction.commit().await.map_err(DatabaseError::Query)?;

        Ok(true)
    }

    /// Counts accounts carrying the system administrator flag.
    ///
    /// # Errors
    ///
    /// Returns [`DatabaseError`] when the count query fails.
    pub async fn system_admin_count(&self) -> Result<i64, DatabaseError> {
        sqlx::query_scalar("SELECT COUNT(*) FROM accounts WHERE is_system_admin = TRUE")
            .fetch_one(&self.pool)
            .await
            .map_err(DatabaseError::Query)
    }

    /// Signs an account in and issues a brand new token for this device.
    ///
    /// Every login creates a new row and a new token: an existing session on another device is
    /// left alone, and no token is ever reused. The returned token is the only time the plaintext
    /// value leaves this function.
    ///
    /// # Errors
    ///
    /// Returns [`DatabaseError`] when the account is unknown, the token cannot be generated, or
    /// the insert fails.
    pub async fn create_session(
        &self,
        email: &EmailAddress,
        password: &str,
        device_label: Option<&str>,
        idle_timeout: IdleTimeout,
        persistent: bool,
        passwords: &PasswordService,
    ) -> Result<IssuedSession, DatabaseError> {
        let account = sqlx::query(
            "SELECT id, password_hash FROM accounts WHERE email = ? AND password_hash IS NOT NULL",
        )
        .bind(email.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(DatabaseError::Query)?
        .ok_or(DatabaseError::InvalidCredentials)?;

        // `password_hash` uses an ascii_bin collation, which MySQL reports as VARBINARY.
        let hash: Vec<u8> = account
            .try_get("password_hash")
            .map_err(DatabaseError::Query)?;
        let hash = String::from_utf8(hash).map_err(|_| DatabaseError::InvalidCredentials)?;
        let hash = PasswordHashValue::parse(hash).map_err(DatabaseError::Password)?;

        if !passwords
            .verify_password(password, &hash)
            .map_err(DatabaseError::Password)?
        {
            return Err(DatabaseError::InvalidCredentials);
        }

        let account_id: Vec<u8> = account.try_get("id").map_err(DatabaseError::Query)?;
        let account_id =
            String::from_utf8(account_id).map_err(|_| DatabaseError::InvalidCredentials)?;

        let lifetime = if persistent {
            SessionLifetime::PERSISTENT_SECONDS
        } else {
            SessionLifetime::DEFAULT_SECONDS
        };

        let token = SessionToken::generate().map_err(|_| DatabaseError::TokenGeneration)?;
        let fingerprint = token.fingerprint();
        let id = Uuid::now_v7().to_string();
        let now = Utc::now();
        let absolute_expires_at = now + Duration::seconds(i64::from(lifetime));

        sqlx::query(
            "INSERT INTO account_sessions \
                 (id, account_id, token_fingerprint, device_label, idle_timeout_seconds, \
                  created_at, last_seen_at, absolute_expires_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&account_id)
        .bind(fingerprint.as_bytes().as_slice())
        .bind(device_label)
        .bind(idle_timeout.stored_seconds())
        .bind(now)
        .bind(now)
        .bind(absolute_expires_at)
        .execute(&self.pool)
        .await
        .map_err(DatabaseError::Query)?;

        Ok(IssuedSession {
            session: SessionRecord {
                id,
                device_label: device_label.map(str::to_owned),
                idle_timeout,
                created_at: now,
                last_seen_at: now,
                absolute_expires_at,
                revoked_at: None,
            },
            token,
        })
    }

    /// Resolves a presented token and refreshes the idle clock in the same statement.
    ///
    /// A session is refused when it is revoked, past its absolute expiry, or idle for longer
    /// than the delay its client chose. Refusal is silent about which of the three applied.
    ///
    /// # Errors
    ///
    /// Returns [`DatabaseError::Session`] with the reason, or [`DatabaseError`] for storage failures.
    pub async fn authenticate_session(
        &self,
        token: &SessionToken,
    ) -> Result<AuthenticatedSession, DatabaseError> {
        let fingerprint = token.fingerprint();
        let now = Utc::now();

        let row = sqlx::query(
            "SELECT account_sessions.id AS session_id, account_id, display_name, \
                    idle_timeout_seconds, last_seen_at, absolute_expires_at, revoked_at \
             FROM account_sessions JOIN accounts ON accounts.id = account_sessions.account_id \
             WHERE token_fingerprint = ?",
        )
        .bind(fingerprint.as_bytes().as_slice())
        .fetch_optional(&self.pool)
        .await
        .map_err(DatabaseError::Query)?
        .ok_or(DatabaseError::Session(SessionError::Unknown))?;

        let revoked_at: Option<DateTime<Utc>> =
            row.try_get("revoked_at").map_err(DatabaseError::Query)?;
        let absolute_expires_at: DateTime<Utc> = row
            .try_get("absolute_expires_at")
            .map_err(DatabaseError::Query)?;
        let last_seen_at: DateTime<Utc> =
            row.try_get("last_seen_at").map_err(DatabaseError::Query)?;
        let idle_timeout_seconds: u32 = row
            .try_get("idle_timeout_seconds")
            .map_err(DatabaseError::Query)?;

        if revoked_at.is_some()
            || now >= absolute_expires_at
            || now - last_seen_at > Duration::seconds(i64::from(idle_timeout_seconds))
        {
            return Err(DatabaseError::Session(SessionError::Expired));
        }

        let session_id: Vec<u8> = row.try_get("session_id").map_err(DatabaseError::Query)?;
        let session_id = String::from_utf8(session_id)
            .map_err(|_| DatabaseError::Session(SessionError::Storage))?;
        let account_id: Vec<u8> = row.try_get("account_id").map_err(DatabaseError::Query)?;
        let account_id = String::from_utf8(account_id)
            .map_err(|_| DatabaseError::Session(SessionError::Storage))?;
        let display_name: String = row.try_get("display_name").map_err(DatabaseError::Query)?;

        sqlx::query("UPDATE account_sessions SET last_seen_at = ? WHERE id = ?")
            .bind(now)
            .bind(&session_id)
            .execute(&self.pool)
            .await
            .map_err(DatabaseError::Query)?;

        Ok(AuthenticatedSession {
            session_id,
            account_id,
            display_name,
            last_seen_at: now,
            absolute_expires_at,
        })
    }

    /// Lists the sessions of one account, newest first, for the client session list (F04).
    ///
    /// # Errors
    ///
    /// Returns [`DatabaseError`] when the query fails.
    pub async fn list_sessions(
        &self,
        account_id: &str,
    ) -> Result<Vec<SessionRecord>, DatabaseError> {
        let rows = sqlx::query(
            "SELECT id, device_label, idle_timeout_seconds, created_at, last_seen_at, \
                    absolute_expires_at, revoked_at \
             FROM account_sessions WHERE account_id = ? ORDER BY created_at DESC",
        )
        .bind(account_id)
        .fetch_all(&self.pool)
        .await
        .map_err(DatabaseError::Query)?;

        rows.into_iter()
            .map(|row| {
                Ok(SessionRecord {
                    id: decode_text(&row, "id")?,
                    device_label: row.try_get("device_label").map_err(DatabaseError::Query)?,
                    idle_timeout: IdleTimeout::from_stored_seconds(
                        row.try_get("idle_timeout_seconds")
                            .map_err(DatabaseError::Query)?,
                    ),
                    created_at: row.try_get("created_at").map_err(DatabaseError::Query)?,
                    last_seen_at: row.try_get("last_seen_at").map_err(DatabaseError::Query)?,
                    absolute_expires_at: row
                        .try_get("absolute_expires_at")
                        .map_err(DatabaseError::Query)?,
                    revoked_at: row.try_get("revoked_at").map_err(DatabaseError::Query)?,
                })
            })
            .collect()
    }

    /// Revokes one session. Revocation is immediate and cannot be undone.
    ///
    /// # Errors
    ///
    /// Returns [`DatabaseError`] when the statement fails.
    pub async fn revoke_session(&self, session_id: &str) -> Result<bool, DatabaseError> {
        let result = sqlx::query(
            "UPDATE account_sessions SET revoked_at = CURRENT_TIMESTAMP(6) \
             WHERE id = ? AND revoked_at IS NULL",
        )
        .bind(session_id)
        .execute(&self.pool)
        .await
        .map_err(DatabaseError::Query)?;

        Ok(result.rows_affected() == 1)
    }
}

/// The identity resolved from a valid session token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticatedSession {
    pub session_id: String,
    pub account_id: String,
    pub display_name: String,
    pub last_seen_at: DateTime<Utc>,
    pub absolute_expires_at: DateTime<Utc>,
}

fn decode_text(row: &sqlx::mysql::MySqlRow, column: &str) -> Result<String, DatabaseError> {
    // Text columns declared `ascii_bin` come back as BINARY/VARBINARY from MySQL.
    let raw: Vec<u8> = row.try_get(column).map_err(DatabaseError::Query)?;
    String::from_utf8(raw).map_err(|_| DatabaseError::Session(SessionError::Storage))
}

#[derive(Debug)]
pub enum DatabaseError {
    Connection(sqlx::Error),
    Migration(sqlx::migrate::MigrateError),
    Query(sqlx::Error),
    Password(crate::credentials::PasswordError),
    TokenGeneration,
    InvalidCredentials,
    Session(SessionError),
}

impl DatabaseError {
    /// Returns the session rejection reason, when this error is one.
    ///
    /// Callers assert on the reason instead of comparing whole errors, because the other
    /// variants wrap foreign error types that cannot be compared.
    #[must_use]
    pub const fn session_error(&self) -> Option<SessionError> {
        match self {
            Self::Session(error) => Some(*error),
            _ => None,
        }
    }
}

impl fmt::Display for DatabaseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connection(error) => write!(formatter, "MariaDB connection failed: {error}"),
            Self::Migration(error) => write!(formatter, "MariaDB migration failed: {error}"),
            Self::Query(error) => write!(formatter, "MariaDB query failed: {error}"),
            Self::Password(_) => {
                formatter.write_str("the bootstrap administrator password is unusable")
            }
            Self::TokenGeneration => formatter.write_str("session token generation failed"),
            Self::InvalidCredentials => formatter.write_str("invalid credentials"),
            Self::Session(_) => formatter.write_str("session rejected"),
        }
    }
}

impl Error for DatabaseError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Connection(error) | Self::Query(error) => Some(error),
            Self::Migration(error) => Some(error),
            Self::Password(error) => Some(error),
            Self::TokenGeneration | Self::InvalidCredentials | Self::Session(_) => None,
        }
    }
}
