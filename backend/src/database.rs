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

/// A collection item, with its server-assigned number and revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub id: String,
    pub space_id: String,
    pub inventory_number: u64,
    pub name: String,
    pub revision: u64,
    pub created_by_account_id: String,
    pub created_at: DateTime<Utc>,
}

/// A recorded move of an item between two spaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemTransfer {
    pub id: String,
    pub item_id: String,
    pub source_space_id: String,
    pub destination_space_id: String,
    pub source_inventory_number: u64,
    pub destination_inventory_number: u64,
    pub transferred_at: DateTime<Utc>,
}

/// The result of a transfer: the updated item plus the history entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferOutcome {
    pub item: Item,
    pub transfer: ItemTransfer,
}

/// Why an inventory operation was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InventoryError {
    /// No item matches the identifier.
    ItemNotFound,
    /// No space matches the identifier.
    SpaceNotFound,
    /// The caller is not the owner of the space it is acting on.
    NotOwner,
    /// The item already lives in the destination space.
    SameSpace,
    /// The presented revision is not the current one.
    RevisionConflict,
    /// The name is empty after trimming, or longer than the column allows.
    InvalidName,
    /// The counter row for the space is missing, which means the space was created without one.
    CounterMissing,
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

    /// Loads the current owner of a space, for the invitation and membership checks.
    ///
    /// # Errors
    ///
    /// Returns [`DatabaseError::Inventory`] with `SpaceNotFound`, or a storage error.
    pub async fn space_owner(&self, space_id: &str) -> Result<String, DatabaseError> {
        let row = sqlx::query("SELECT owner_account_id FROM spaces WHERE id = ?")
            .bind(space_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(DatabaseError::Query)?
            .ok_or(DatabaseError::Inventory(InventoryError::SpaceNotFound))?;

        decode_text(&row, "owner_account_id")
    }

    /// Creates an item and reserves its number in one transaction.
    ///
    /// The number is never supplied by the caller. The space counter row is locked with
    /// `SELECT ... FOR UPDATE`, so two concurrent creations cannot read the same value, and
    /// the unique key on `(space_id, inventory_number)` is the second line of defence.
    /// A failure rolls back the reservation, so a number is never burned by a failed attempt.
    ///
    /// # Errors
    ///
    /// Returns [`DatabaseError::Inventory`] for an empty or oversized name, an unknown space,
    /// or a missing counter row.
    pub async fn create_item(
        &self,
        space_id: &str,
        name: &str,
        created_by_account_id: &str,
    ) -> Result<Item, DatabaseError> {
        let name = name.trim();

        if name.is_empty() || name.chars().count() > MAX_ITEM_NAME_CHARS {
            return Err(DatabaseError::Inventory(InventoryError::InvalidName));
        }

        let mut transaction = self.pool.begin().await.map_err(DatabaseError::Query)?;

        // Lock first, read second: the lock must be held before the value is read, otherwise
        // two transactions could both read the same number and collide on the unique key.
        let next_number: Option<u64> = sqlx::query_scalar(
            "SELECT next_number FROM inventory_counters WHERE space_id = ? FOR UPDATE",
        )
        .bind(space_id)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(DatabaseError::Query)?;

        let Some(next_number) = next_number else {
            transaction.rollback().await.map_err(DatabaseError::Query)?;
            return Err(DatabaseError::Inventory(InventoryError::CounterMissing));
        };

        sqlx::query(
            "UPDATE inventory_counters SET next_number = next_number + 1 WHERE space_id = ?",
        )
        .bind(space_id)
        .execute(&mut *transaction)
        .await
        .map_err(DatabaseError::Query)?;

        let id = Uuid::now_v7().to_string();
        sqlx::query(
            "INSERT INTO inventory_items \
                 (id, space_id, inventory_number, name, revision, created_by_account_id) \
             VALUES (?, ?, ?, ?, 1, ?)",
        )
        .bind(&id)
        .bind(space_id)
        .bind(next_number)
        .bind(name)
        .bind(created_by_account_id)
        .execute(&mut *transaction)
        .await
        .map_err(DatabaseError::Query)?;

        let item = fetch_item(&mut *transaction, &id).await?;
        transaction.commit().await.map_err(DatabaseError::Query)?;

        Ok(item)
    }

    /// Loads an item by its stable identifier.
    ///
    /// # Errors
    ///
    /// Returns [`DatabaseError::Inventory`] with `ItemNotFound`, or a storage error.
    pub async fn item(&self, item_id: &str) -> Result<Item, DatabaseError> {
        fetch_item(&self.pool, item_id).await
    }

    /// Moves an item to another space, renumbering it there.
    ///
    /// The item row is locked first so two concurrent transfers of the same item are
    /// serialized: the second one sees the new revision and is refused. The source and
    /// destination numbers are both recorded, so the history keeps the old number after the
    /// item has moved. The whole operation is one transaction.
    ///
    /// # Errors
    ///
    /// Returns [`DatabaseError::Inventory`] for an unknown item or space, a stale revision,
    /// a destination identical to the source, or a missing destination counter.
    pub async fn transfer_item(
        &self,
        item_id: &str,
        destination_space_id: &str,
        expected_revision: u64,
        actor_account_id: &str,
    ) -> Result<TransferOutcome, DatabaseError> {
        let mut transaction = self.pool.begin().await.map_err(DatabaseError::Query)?;

        let row = sqlx::query(
            "SELECT space_id, inventory_number, revision \
             FROM inventory_items WHERE id = ? FOR UPDATE",
        )
        .bind(item_id)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(DatabaseError::Query)?
        .ok_or(DatabaseError::Inventory(InventoryError::ItemNotFound))?;

        let source_space_id = decode_text(&row, "space_id")?;
        let source_inventory_number: u64 = row
            .try_get("inventory_number")
            .map_err(DatabaseError::Query)?;
        let revision: u64 = row.try_get("revision").map_err(DatabaseError::Query)?;

        if revision != expected_revision {
            transaction.rollback().await.map_err(DatabaseError::Query)?;
            return Err(DatabaseError::Inventory(InventoryError::RevisionConflict));
        }

        if source_space_id == destination_space_id {
            transaction.rollback().await.map_err(DatabaseError::Query)?;
            return Err(DatabaseError::Inventory(InventoryError::SameSpace));
        }

        let destination_next: Option<u64> = sqlx::query_scalar(
            "SELECT next_number FROM inventory_counters WHERE space_id = ? FOR UPDATE",
        )
        .bind(destination_space_id)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(DatabaseError::Query)?;

        let Some(destination_number) = destination_next else {
            transaction.rollback().await.map_err(DatabaseError::Query)?;
            return Err(DatabaseError::Inventory(InventoryError::CounterMissing));
        };

        sqlx::query(
            "UPDATE inventory_counters SET next_number = next_number + 1 WHERE space_id = ?",
        )
        .bind(destination_space_id)
        .execute(&mut *transaction)
        .await
        .map_err(DatabaseError::Query)?;

        sqlx::query(
            "UPDATE inventory_items \
             SET space_id = ?, inventory_number = ?, revision = revision + 1 \
             WHERE id = ?",
        )
        .bind(destination_space_id)
        .bind(destination_number)
        .bind(item_id)
        .execute(&mut *transaction)
        .await
        .map_err(DatabaseError::Query)?;

        let transfer_id = Uuid::now_v7().to_string();
        sqlx::query(
            "INSERT INTO inventory_transfers \
                 (id, item_id, source_space_id, destination_space_id, source_inventory_number, \
                  destination_inventory_number, actor_account_id) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&transfer_id)
        .bind(item_id)
        .bind(&source_space_id)
        .bind(destination_space_id)
        .bind(source_inventory_number)
        .bind(destination_number)
        .bind(actor_account_id)
        .execute(&mut *transaction)
        .await
        .map_err(DatabaseError::Query)?;

        let item = fetch_item(&mut *transaction, item_id).await?;
        let transfer = fetch_transfer(&mut *transaction, &transfer_id).await?;
        transaction.commit().await.map_err(DatabaseError::Query)?;

        Ok(TransferOutcome { item, transfer })
    }

    /// Lists the transfers of an item, newest first, for the item history.
    ///
    /// # Errors
    ///
    /// Returns [`DatabaseError`] when the query fails.
    pub async fn item_transfers(&self, item_id: &str) -> Result<Vec<ItemTransfer>, DatabaseError> {
        let rows = sqlx::query(
            "SELECT id, item_id, source_space_id, destination_space_id, source_inventory_number, \
                    destination_inventory_number, transferred_at \
             FROM inventory_transfers WHERE item_id = ? ORDER BY transferred_at DESC, id DESC",
        )
        .bind(item_id)
        .fetch_all(&self.pool)
        .await
        .map_err(DatabaseError::Query)?;

        rows.iter().map(decode_transfer).collect()
    }
}

/// Maximum length of an item name, aligned with the `VARCHAR(255)` column.
pub const MAX_ITEM_NAME_CHARS: usize = 255;

async fn fetch_item<'e, E>(executor: E, item_id: &str) -> Result<Item, DatabaseError>
where
    E: sqlx::Executor<'e, Database = sqlx::MySql>,
{
    let row = sqlx::query(
        "SELECT id, space_id, inventory_number, name, revision, created_by_account_id, created_at \
         FROM inventory_items WHERE id = ?",
    )
    .bind(item_id)
    .fetch_optional(executor)
    .await
    .map_err(DatabaseError::Query)?
    .ok_or(DatabaseError::Inventory(InventoryError::ItemNotFound))?;

    Ok(Item {
        id: decode_text(&row, "id")?,
        space_id: decode_text(&row, "space_id")?,
        inventory_number: row
            .try_get("inventory_number")
            .map_err(DatabaseError::Query)?,
        name: row.try_get("name").map_err(DatabaseError::Query)?,
        revision: row.try_get("revision").map_err(DatabaseError::Query)?,
        created_by_account_id: decode_text(&row, "created_by_account_id")?,
        created_at: row.try_get("created_at").map_err(DatabaseError::Query)?,
    })
}

async fn fetch_transfer<'e, E>(
    executor: E,
    transfer_id: &str,
) -> Result<ItemTransfer, DatabaseError>
where
    E: sqlx::Executor<'e, Database = sqlx::MySql>,
{
    let row = sqlx::query(
        "SELECT id, item_id, source_space_id, destination_space_id, source_inventory_number, \
                destination_inventory_number, transferred_at \
         FROM inventory_transfers WHERE id = ?",
    )
    .bind(transfer_id)
    .fetch_optional(executor)
    .await
    .map_err(DatabaseError::Query)?
    .ok_or(DatabaseError::Inventory(InventoryError::ItemNotFound))?;

    decode_transfer(&row)
}

fn decode_transfer(row: &sqlx::mysql::MySqlRow) -> Result<ItemTransfer, DatabaseError> {
    Ok(ItemTransfer {
        id: decode_text(row, "id")?,
        item_id: decode_text(row, "item_id")?,
        source_space_id: decode_text(row, "source_space_id")?,
        destination_space_id: decode_text(row, "destination_space_id")?,
        source_inventory_number: row
            .try_get("source_inventory_number")
            .map_err(DatabaseError::Query)?,
        destination_inventory_number: row
            .try_get("destination_inventory_number")
            .map_err(DatabaseError::Query)?,
        transferred_at: row
            .try_get("transferred_at")
            .map_err(DatabaseError::Query)?,
    })
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
    Inventory(InventoryError),
    Migration(sqlx::migrate::MigrateError),
    Query(sqlx::Error),
    Password(crate::credentials::PasswordError),
    TokenGeneration,
    InvalidCredentials,
    Session(SessionError),
}

impl DatabaseError {
    /// Returns the inventory rejection reason, when this error is one.
    #[must_use]
    pub const fn inventory_error(&self) -> Option<InventoryError> {
        match self {
            Self::Inventory(error) => Some(*error),
            _ => None,
        }
    }

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
            Self::Inventory(error) => write!(formatter, "inventory rejected: {error:?}"),
        }
    }
}

impl Error for DatabaseError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Connection(error) | Self::Query(error) => Some(error),
            Self::Migration(error) => Some(error),
            Self::Password(error) => Some(error),
            Self::TokenGeneration
            | Self::InvalidCredentials
            | Self::Session(_)
            | Self::Inventory(_) => None,
        }
    }
}
