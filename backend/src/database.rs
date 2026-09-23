use std::{collections::BTreeSet, error::Error, fmt};

use chrono::{DateTime, Duration, Utc};
use sqlx::{MySqlPool, Row, mysql::MySqlPoolOptions};
use uuid::Uuid;

use crate::{
    config::BootstrapAdmin,
    credentials::{
        EmailAddress, IdleTimeout, PasswordHashValue, PasswordService, SessionLifetime,
        SessionToken,
    },
    security::{Permission, SpaceMembership, SpaceOwnership},
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
    pub account_id: Uuid,
    pub display_name: String,
    pub is_system_admin: bool,
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

/// The lifecycle state of an inventory item.
///
/// Every state is reversible: `archived` hides the item from the current view, `trashed` is the
/// trash, and `active` is the normal working state. No state removes the item from the database,
/// because the physical retention rules are not decided yet. A `trashed` item is read-only until
/// it is restored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemState {
    Active,
    Archived,
    Trashed,
}

impl ItemState {
    /// The stable code persisted in MariaDB and exposed by the API.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Archived => "archived",
            Self::Trashed => "trashed",
        }
    }

    /// Parses a persisted or query code, returning `None` for an unknown value.
    #[must_use]
    pub fn from_code(code: &str) -> Option<Self> {
        match code {
            "active" => Some(Self::Active),
            "archived" => Some(Self::Archived),
            "trashed" => Some(Self::Trashed),
            _ => None,
        }
    }

    /// Returns whether a transition from `self` to `target` is one of the allowed moves.
    #[must_use]
    pub const fn can_transition_to(self, target: Self) -> bool {
        matches!(
            (self, target),
            (Self::Active, Self::Archived | Self::Trashed)
                | (Self::Archived, Self::Active | Self::Trashed)
                | (Self::Trashed, Self::Active)
        )
    }
}

/// A collection item, with its server-assigned number, state and revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub id: Uuid,
    pub space_id: Uuid,
    pub inventory_number: u64,
    pub name: String,
    pub description: Option<String>,
    pub historical_reference: Option<String>,
    pub technical_reference: Option<String>,
    pub state: ItemState,
    pub revision: u64,
    pub created_by_account_id: Uuid,
    pub created_at: DateTime<Utc>,
}

/// A recorded move of an item between two spaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemTransfer {
    pub id: Uuid,
    pub item_id: Uuid,
    pub source_space_id: Uuid,
    pub destination_space_id: Uuid,
    pub source_inventory_number: u64,
    pub destination_inventory_number: u64,
    pub transferred_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ItemSearchFilters {
    pub category_id: Option<Uuid>,
    pub location_id: Option<Uuid>,
    pub group_id: Option<Uuid>,
}

/// An audited lifecycle change made while the item belonged to a space.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemStateEvent {
    pub id: Uuid,
    pub item_id: Uuid,
    pub space_id: Uuid,
    pub actor_account_id: Uuid,
    pub actor_display_name: String,
    pub state_before: ItemState,
    pub state_after: ItemState,
    pub created_at: DateTime<Utc>,
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
    /// A free-text detail exceeds the allowed length.
    InvalidDetails,
    /// The counter row for the space is missing, which means the space was created without one.
    CounterMissing,
    /// The item state forbids this operation, for example renaming or transferring a trashed item.
    InvalidState,
    /// The requested state transition is not allowed from the current state.
    InvalidTransition,
    /// Categorized items require an explicit destination classification on transfer.
    DestinationCategoriesRequired,
    /// A selected destination category does not belong to the destination space.
    InvalidDestinationCategory,
}

/// A space and its current owner.
///
/// Identifiers are UUIDs for the same reason as [`Membership`]: the space can be handed to an
/// authorization check without parsing it again at every call site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Space {
    pub id: Uuid,
    pub name: String,
    pub owner_account_id: Uuid,
    pub created_at: DateTime<Utc>,
}

impl Space {
    /// Borrows the space in the shape the ownership check expects.
    #[must_use]
    pub fn as_space_ownership(&self) -> SpaceOwnership {
        SpaceOwnership {
            space_id: self.id,
            owner_account_id: self.owner_account_id,
        }
    }
}

/// A membership of one account in one space, with the account's effective grants.
///
/// Identifiers are UUIDs and grants are a set, so this value can be handed straight to
/// [`crate::security::Principal::require_space_permission`] without a lossy conversion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Membership {
    pub space_id: Uuid,
    pub account_id: Uuid,
    /// Explicit grants, sorted. Never inferred from another grant or from the owner status.
    pub permissions: BTreeSet<Permission>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberWithAccount {
    pub membership: Membership,
    pub display_name: String,
    pub email: Option<String>,
}

impl Membership {
    /// Borrows this membership in the shape the authorization check expects.
    #[must_use]
    pub fn as_space_membership(&self) -> SpaceMembership {
        SpaceMembership {
            space_id: self.space_id,
            account_id: self.account_id,
            permissions: self.permissions.clone(),
        }
    }
}

/// Why a space or membership operation was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpaceError {
    /// No space matches the identifier.
    SpaceNotFound,
    /// No account matches the identifier.
    AccountNotFound,
    /// The name is empty after trimming, or longer than the column allows.
    InvalidName,
    /// A grant is not a space-scoped permission, so it cannot live in a membership.
    PermissionNotSpaceScoped,
    /// The account is already a member of this space.
    AlreadyMember,
    /// The account is not a member of this space.
    NotMember,
    /// The owner cannot be removed from its own space, because a space has exactly one owner.
    OwnerCannotBeRemoved,
    /// Changing your own grants would let you grant yourself more than you hold.
    CannotChangeOwnGrants,
    /// Membership administration requires the current owner or a system administrator.
    NotAuthorized,
    /// The owner must retain collection read and write grants.
    OwnerCoreGrantsRequired,
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
            "SELECT id, display_name, password_hash, is_system_admin FROM accounts WHERE email = ? AND password_hash IS NOT NULL",
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
        let account_uuid = Uuid::parse_str(&account_id)
            .map_err(|_| DatabaseError::Session(SessionError::Storage))?;
        let display_name: String = account
            .try_get("display_name")
            .map_err(DatabaseError::Query)?;
        let is_system_admin: bool = account
            .try_get("is_system_admin")
            .map_err(DatabaseError::Query)?;

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
        .bind(account_id.clone())
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
            account_id: account_uuid,
            display_name,
            is_system_admin,
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
            "SELECT account_sessions.id AS session_id, account_id, display_name, is_system_admin, \
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
        let is_system_admin: bool = row
            .try_get("is_system_admin")
            .map_err(DatabaseError::Query)?;

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
            is_system_admin,
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
        .bind(account_id.to_string())
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
    pub async fn space_owner(&self, space_id: Uuid) -> Result<Uuid, DatabaseError> {
        let row = sqlx::query("SELECT owner_account_id FROM spaces WHERE id = ?")
            .bind(space_id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(DatabaseError::Query)?
            .ok_or(DatabaseError::Inventory(InventoryError::SpaceNotFound))?;

        parse_uuid(&decode_text(&row, "owner_account_id")?)
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
        space_id: Uuid,
        name: &str,
        created_by_account_id: Uuid,
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
        .bind(space_id.to_string())
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
        .bind(space_id.to_string())
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
        .bind(space_id.to_string())
        .bind(next_number)
        .bind(name)
        .bind(created_by_account_id.to_string())
        .execute(&mut *transaction)
        .await
        .map_err(DatabaseError::Query)?;

        let item = fetch_item(&mut *transaction, parse_uuid(&id)?).await?;
        transaction.commit().await.map_err(DatabaseError::Query)?;

        Ok(item)
    }

    /// Loads an item by its stable identifier.
    ///
    /// # Errors
    ///
    /// Returns [`DatabaseError::Inventory`] with `ItemNotFound`, or a storage error.
    pub async fn item(&self, item_id: Uuid) -> Result<Item, DatabaseError> {
        fetch_item(&self.pool, item_id).await
    }

    /// Renames an item if its revision still matches the client's copy.
    ///
    /// # Errors
    ///
    /// Returns an inventory error for an invalid name, a missing item, or a stale revision.
    pub async fn rename_item(
        &self,
        item_id: Uuid,
        name: &str,
        expected_revision: u64,
    ) -> Result<Item, DatabaseError> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > MAX_ITEM_NAME_CHARS {
            return Err(DatabaseError::Inventory(InventoryError::InvalidName));
        }

        let mut transaction = self.pool.begin().await.map_err(DatabaseError::Query)?;
        let row =
            sqlx::query("SELECT state, revision FROM inventory_items WHERE id = ? FOR UPDATE")
                .bind(item_id.to_string())
                .fetch_optional(&mut *transaction)
                .await
                .map_err(DatabaseError::Query)?
                .ok_or(DatabaseError::Inventory(InventoryError::ItemNotFound))?;
        let revision: u64 = row.try_get("revision").map_err(DatabaseError::Query)?;
        if revision != expected_revision {
            transaction.rollback().await.map_err(DatabaseError::Query)?;
            return Err(DatabaseError::Inventory(InventoryError::RevisionConflict));
        }
        let current = decode_state(&row)?;
        if current == ItemState::Trashed {
            transaction.rollback().await.map_err(DatabaseError::Query)?;
            return Err(DatabaseError::Inventory(InventoryError::InvalidState));
        }
        sqlx::query("UPDATE inventory_items SET name = ?, revision = revision + 1 WHERE id = ?")
            .bind(name)
            .bind(item_id.to_string())
            .execute(&mut *transaction)
            .await
            .map_err(DatabaseError::Query)?;
        let item = fetch_item(&mut *transaction, item_id).await?;
        transaction.commit().await.map_err(DatabaseError::Query)?;
        Ok(item)
    }

    /// Changes an item's lifecycle state, refusing a stale revision or an invalid transition.
    ///
    /// The item row is locked so two concurrent transitions are serialized: the loser sees the
    /// new revision and is refused. Every transition increments the revision and appends an audit
    /// event with the actor and the exact before/after state. The whole operation is one
    /// transaction, so a refused transition records nothing.
    ///
    /// # Errors
    ///
    /// Returns [`DatabaseError::Inventory`] for an unknown item, a stale revision, or a transition
    /// that the current state does not allow.
    pub async fn set_item_state(
        &self,
        item_id: Uuid,
        target: ItemState,
        expected_revision: u64,
        actor_account_id: Uuid,
    ) -> Result<Item, DatabaseError> {
        let mut transaction = self.pool.begin().await.map_err(DatabaseError::Query)?;
        let row = sqlx::query(
            "SELECT space_id, state, revision FROM inventory_items WHERE id = ? FOR UPDATE",
        )
        .bind(item_id.to_string())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(DatabaseError::Query)?
        .ok_or(DatabaseError::Inventory(InventoryError::ItemNotFound))?;

        let revision: u64 = row.try_get("revision").map_err(DatabaseError::Query)?;
        if revision != expected_revision {
            transaction.rollback().await.map_err(DatabaseError::Query)?;
            return Err(DatabaseError::Inventory(InventoryError::RevisionConflict));
        }

        let space_id = parse_uuid(&decode_text(&row, "space_id")?)?;
        let current = decode_state(&row)?;
        if !current.can_transition_to(target) {
            transaction.rollback().await.map_err(DatabaseError::Query)?;
            return Err(DatabaseError::Inventory(InventoryError::InvalidTransition));
        }

        sqlx::query("UPDATE inventory_items SET state = ?, revision = revision + 1 WHERE id = ?")
            .bind(target.as_str())
            .bind(item_id.to_string())
            .execute(&mut *transaction)
            .await
            .map_err(DatabaseError::Query)?;

        insert_item_state_audit(
            &mut transaction,
            item_id,
            space_id,
            actor_account_id,
            current,
            target,
        )
        .await?;

        let item = fetch_item(&mut *transaction, item_id).await?;
        transaction.commit().await.map_err(DatabaseError::Query)?;
        Ok(item)
    }

    /// Replaces the descriptive fields under the same optimistic lock as other item edits.
    ///
    /// # Errors
    ///
    /// Returns a validation, missing item, stale revision, forbidden state, or storage error.
    pub async fn update_item_details(
        &self,
        item_id: Uuid,
        description: Option<&str>,
        historical_reference: Option<&str>,
        technical_reference: Option<&str>,
        expected_revision: u64,
    ) -> Result<Item, DatabaseError> {
        fn normalized(value: Option<&str>, max: usize) -> Result<Option<String>, DatabaseError> {
            let value = value.map(str::trim).filter(|value| !value.is_empty());
            if value.is_some_and(|value| value.chars().count() > max) {
                return Err(DatabaseError::Inventory(InventoryError::InvalidDetails));
            }
            Ok(value.map(str::to_owned))
        }
        let description = normalized(description, 10_000)?;
        let historical_reference = normalized(historical_reference, 500)?;
        let technical_reference = normalized(technical_reference, 500)?;
        let mut transaction = self.pool.begin().await.map_err(DatabaseError::Query)?;
        let row =
            sqlx::query("SELECT state, revision FROM inventory_items WHERE id = ? FOR UPDATE")
                .bind(item_id.to_string())
                .fetch_optional(&mut *transaction)
                .await
                .map_err(DatabaseError::Query)?
                .ok_or(DatabaseError::Inventory(InventoryError::ItemNotFound))?;
        let revision: u64 = row.try_get("revision").map_err(DatabaseError::Query)?;
        if revision != expected_revision {
            return Err(DatabaseError::Inventory(InventoryError::RevisionConflict));
        }
        if decode_state(&row)? == ItemState::Trashed {
            return Err(DatabaseError::Inventory(InventoryError::InvalidState));
        }
        sqlx::query(
            "UPDATE inventory_items SET description = ?, historical_reference = ?, \
                     technical_reference = ?, revision = revision + 1 WHERE id = ?",
        )
        .bind(description)
        .bind(historical_reference)
        .bind(technical_reference)
        .bind(item_id.to_string())
        .execute(&mut *transaction)
        .await
        .map_err(DatabaseError::Query)?;
        let item = fetch_item(&mut *transaction, item_id).await?;
        transaction.commit().await.map_err(DatabaseError::Query)?;
        Ok(item)
    }

    /// Lists lifecycle audit events created in the item's current space.
    ///
    /// Events from previous spaces are deliberately excluded: moving an item never transfers
    /// permission to inspect the previous space's audit log.
    ///
    /// # Errors
    ///
    /// Returns a storage error for a failed query or malformed persisted data.
    pub async fn item_state_events(
        &self,
        item_id: Uuid,
        space_id: Uuid,
    ) -> Result<Vec<ItemStateEvent>, DatabaseError> {
        let rows = sqlx::query(
            "SELECT e.id, e.item_id, e.space_id, e.actor_account_id, a.display_name AS actor_display_name, \
                    e.state_before, e.state_after, e.created_at \
             FROM inventory_item_audit_events e \
             JOIN accounts a ON a.id = e.actor_account_id \
             WHERE e.item_id = ? AND e.space_id = ? \
             ORDER BY e.created_at DESC, e.id DESC",
        )
        .bind(item_id.to_string())
        .bind(space_id.to_string())
        .fetch_all(&self.pool)
        .await
        .map_err(DatabaseError::Query)?;

        rows.iter()
            .map(|row| {
                let before = ItemState::from_code(&decode_text(row, "state_before")?)
                    .ok_or(DatabaseError::Inventory(InventoryError::InvalidState))?;
                let after = ItemState::from_code(&decode_text(row, "state_after")?)
                    .ok_or(DatabaseError::Inventory(InventoryError::InvalidState))?;
                Ok(ItemStateEvent {
                    id: parse_uuid(&decode_text(row, "id")?)?,
                    item_id: parse_uuid(&decode_text(row, "item_id")?)?,
                    space_id: parse_uuid(&decode_text(row, "space_id")?)?,
                    actor_account_id: parse_uuid(&decode_text(row, "actor_account_id")?)?,
                    actor_display_name: row
                        .try_get("actor_display_name")
                        .map_err(DatabaseError::Query)?,
                    state_before: before,
                    state_after: after,
                    created_at: row.try_get("created_at").map_err(DatabaseError::Query)?,
                })
            })
            .collect()
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
        item_id: Uuid,
        destination_space_id: Uuid,
        expected_revision: u64,
        actor_account_id: Uuid,
    ) -> Result<TransferOutcome, DatabaseError> {
        self.transfer_item_with_categories(
            item_id,
            destination_space_id,
            expected_revision,
            actor_account_id,
            &[],
        )
        .await
    }

    /// Transfers an item and explicitly classifies it in the destination space in the same
    /// transaction. Old assignments and values remain as historical records.
    ///
    /// # Errors
    ///
    /// Returns an inventory or storage error; categorized source items cannot be transferred
    /// without at least one valid destination category.
    pub async fn transfer_item_with_categories(
        &self,
        item_id: Uuid,
        destination_space_id: Uuid,
        expected_revision: u64,
        actor_account_id: Uuid,
        destination_category_ids: &[Uuid],
    ) -> Result<TransferOutcome, DatabaseError> {
        let mut transaction = self.pool.begin().await.map_err(DatabaseError::Query)?;

        let row = sqlx::query(
            "SELECT space_id, inventory_number, current_location_id, state, revision \
             FROM inventory_items WHERE id = ? FOR UPDATE",
        )
        .bind(item_id.to_string())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(DatabaseError::Query)?
        .ok_or(DatabaseError::Inventory(InventoryError::ItemNotFound))?;

        let source_space_id = parse_uuid(&decode_text(&row, "space_id")?)?;
        let source_inventory_number: u64 = row
            .try_get("inventory_number")
            .map_err(DatabaseError::Query)?;
        let revision: u64 = row.try_get("revision").map_err(DatabaseError::Query)?;

        if revision != expected_revision {
            transaction.rollback().await.map_err(DatabaseError::Query)?;
            return Err(DatabaseError::Inventory(InventoryError::RevisionConflict));
        }

        if decode_state(&row)? == ItemState::Trashed {
            transaction.rollback().await.map_err(DatabaseError::Query)?;
            return Err(DatabaseError::Inventory(InventoryError::InvalidState));
        }

        if source_space_id == destination_space_id {
            transaction.rollback().await.map_err(DatabaseError::Query)?;
            return Err(DatabaseError::Inventory(InventoryError::SameSpace));
        }

        let destination_next: Option<u64> = sqlx::query_scalar(
            "SELECT next_number FROM inventory_counters WHERE space_id = ? FOR UPDATE",
        )
        .bind(destination_space_id.to_string())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(DatabaseError::Query)?;

        let Some(destination_number) = destination_next else {
            transaction.rollback().await.map_err(DatabaseError::Query)?;
            return Err(DatabaseError::Inventory(InventoryError::CounterMissing));
        };

        crate::taxonomy::apply_transfer_categories(
            &mut transaction,
            item_id,
            source_space_id,
            destination_space_id,
            destination_category_ids,
        )
        .await?;

        crate::groups::clear_item_groups_on_transfer(&mut transaction, item_id).await?;

        crate::locations::record_transfer_location_exit(
            &mut transaction,
            item_id,
            source_space_id,
            row.try_get("current_location_id")
                .map_err(DatabaseError::Query)?,
            actor_account_id,
        )
        .await?;

        sqlx::query(
            "UPDATE inventory_counters SET next_number = next_number + 1 WHERE space_id = ?",
        )
        .bind(destination_space_id.to_string())
        .execute(&mut *transaction)
        .await
        .map_err(DatabaseError::Query)?;

        sqlx::query(
            "UPDATE inventory_items \
             SET space_id = ?, inventory_number = ?, current_location_id = NULL, revision = revision + 1 \
             WHERE id = ?",
        )
        .bind(destination_space_id.to_string())
        .bind(destination_number)
        .bind(item_id.to_string())
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
        .bind(item_id.to_string())
        .bind(source_space_id.to_string())
        .bind(destination_space_id.to_string())
        .bind(source_inventory_number)
        .bind(destination_number)
        .bind(actor_account_id.to_string())
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
    pub async fn item_transfers(&self, item_id: Uuid) -> Result<Vec<ItemTransfer>, DatabaseError> {
        let rows = sqlx::query(
            "SELECT id, item_id, source_space_id, destination_space_id, source_inventory_number, \
                    destination_inventory_number, transferred_at \
             FROM inventory_transfers WHERE item_id = ? ORDER BY transferred_at DESC, id DESC",
        )
        .bind(item_id.to_string())
        .fetch_all(&self.pool)
        .await
        .map_err(DatabaseError::Query)?;

        rows.iter().map(decode_transfer).collect()
    }

    /// Creates a space, its owner membership and its inventory counter in one transaction.
    ///
    /// A space without a counter row could not accept a single item, and a space without an
    /// owner membership would be visible only to the owner through the ownership rule. Both
    /// invariants are established here rather than left to callers.
    ///
    /// The owner receives `collections_write`, not every permission: the owner is not
    /// automatically granted financial access, and nothing is ever inferred from ownership.
    ///
    /// # Errors
    ///
    /// Returns [`DatabaseError::Space`] for an empty or oversized name, an unknown owner
    /// account, or a conflicting membership.
    pub async fn create_space(
        &self,
        name: &str,
        owner_account_id: Uuid,
    ) -> Result<Space, DatabaseError> {
        let name = name.trim();

        if name.is_empty() || name.chars().count() > MAX_SPACE_NAME_CHARS {
            return Err(DatabaseError::Space(SpaceError::InvalidName));
        }

        let mut transaction = self.pool.begin().await.map_err(DatabaseError::Query)?;

        let owner_exists: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM accounts WHERE id = ?")
            .bind(owner_account_id.to_string())
            .fetch_one(&mut *transaction)
            .await
            .map_err(DatabaseError::Query)?;

        if owner_exists == 0 {
            transaction.rollback().await.map_err(DatabaseError::Query)?;
            return Err(DatabaseError::Space(SpaceError::AccountNotFound));
        }

        let space_id = Uuid::now_v7();

        sqlx::query("INSERT INTO spaces (id, name, owner_account_id) VALUES (?, ?, ?)")
            .bind(space_id.to_string())
            .bind(name)
            .bind(owner_account_id.to_string())
            .execute(&mut *transaction)
            .await
            .map_err(DatabaseError::Query)?;

        sqlx::query("INSERT INTO inventory_counters (space_id, next_number) VALUES (?, 1)")
            .bind(space_id.to_string())
            .execute(&mut *transaction)
            .await
            .map_err(DatabaseError::Query)?;

        sqlx::query("INSERT INTO space_memberships (space_id, account_id) VALUES (?, ?)")
            .bind(space_id.to_string())
            .bind(owner_account_id.to_string())
            .execute(&mut *transaction)
            .await
            .map_err(DatabaseError::Query)?;

        sqlx::query(
            "INSERT INTO space_permission_grants (space_id, account_id, permission_code) \
             VALUES (?, ?, ?)",
        )
        .bind(space_id.to_string())
        .bind(owner_account_id.to_string())
        .bind(permission_code(Permission::CollectionsWrite))
        .execute(&mut *transaction)
        .await
        .map_err(DatabaseError::Query)?;

        sqlx::query(
            "INSERT INTO space_permission_grants (space_id, account_id, permission_code) \
             VALUES (?, ?, ?)",
        )
        .bind(space_id.to_string())
        .bind(owner_account_id.to_string())
        .bind(permission_code(Permission::CollectionsRead))
        .execute(&mut *transaction)
        .await
        .map_err(DatabaseError::Query)?;

        for permission in [Permission::AcquisitionsRead, Permission::AcquisitionsWrite] {
            sqlx::query(
                "INSERT INTO space_permission_grants (space_id, account_id, permission_code) \
                 VALUES (?, ?, ?)",
            )
            .bind(space_id.to_string())
            .bind(owner_account_id.to_string())
            .bind(permission_code(permission))
            .execute(&mut *transaction)
            .await
            .map_err(DatabaseError::Query)?;
        }

        let space = fetch_space(&mut *transaction, space_id).await?;
        transaction.commit().await.map_err(DatabaseError::Query)?;

        Ok(space)
    }

    /// Loads a space by its identifier.
    ///
    /// # Errors
    ///
    /// Returns [`DatabaseError::Space`] with `SpaceNotFound`, or a storage error.
    pub async fn space(&self, space_id: Uuid) -> Result<Space, DatabaseError> {
        fetch_space(&self.pool, space_id).await
    }

    /// Lists only spaces where the account has an explicit membership.
    ///
    /// # Errors
    ///
    /// Returns a storage error if a space cannot be read.
    pub async fn spaces_for_account(&self, account_id: Uuid) -> Result<Vec<Space>, DatabaseError> {
        let rows = sqlx::query(
            "SELECT s.id FROM spaces s JOIN space_memberships m ON m.space_id = s.id \
             WHERE m.account_id = ? ORDER BY s.created_at, s.id",
        )
        .bind(account_id.to_string())
        .fetch_all(&self.pool)
        .await
        .map_err(DatabaseError::Query)?;

        let mut spaces = Vec::with_capacity(rows.len());
        for row in rows {
            spaces.push(self.space(parse_uuid(&decode_text(&row, "id")?)?).await?);
        }
        Ok(spaces)
    }

    /// Lists all spaces for administration; callers must enforce system-admin authorization.
    ///
    /// # Errors
    ///
    /// Returns a storage error when the spaces cannot be loaded.
    pub async fn all_spaces(&self) -> Result<Vec<Space>, DatabaseError> {
        let rows = sqlx::query("SELECT id FROM spaces ORDER BY created_at, id")
            .fetch_all(&self.pool)
            .await
            .map_err(DatabaseError::Query)?;
        let mut spaces = Vec::with_capacity(rows.len());
        for row in &rows {
            let id = parse_uuid(&decode_text(row, "id")?)?;
            spaces.push(fetch_space(&self.pool, id).await?);
        }
        Ok(spaces)
    }

    /// Lists inventory in one space; the HTTP layer must authorize that space first.
    ///
    /// # Errors
    ///
    /// Returns a storage error if an item cannot be read.
    pub async fn items_in_space(&self, space_id: Uuid) -> Result<Vec<Item>, DatabaseError> {
        let rows = sqlx::query(
            "SELECT id FROM inventory_items WHERE space_id = ? ORDER BY inventory_number",
        )
        .bind(space_id.to_string())
        .fetch_all(&self.pool)
        .await
        .map_err(DatabaseError::Query)?;

        let mut items = Vec::with_capacity(rows.len());
        for row in rows {
            items.push(self.item(parse_uuid(&decode_text(&row, "id")?)?).await?);
        }
        Ok(items)
    }

    /// Returns at most `limit` items after an inventory number, optionally matching a name and
    /// restricted to one lifecycle state, category subtree and physical-location subtree.
    /// `state` is a validated code or `all`.
    /// The caller requests one extra row to determine whether another page exists.
    ///
    /// # Errors
    ///
    /// Returns a storage error if the query or row decoding fails.
    pub async fn search_items_in_space(
        &self,
        space_id: Uuid,
        search: &str,
        after_inventory_number: u64,
        state: &str,
        limit: u32,
        filters: ItemSearchFilters,
    ) -> Result<Vec<Item>, DatabaseError> {
        let rows = sqlx::query(
            "WITH RECURSIVE category_tree AS ( \
                 SELECT id FROM inventory_categories WHERE id = ? AND space_id = ? \
                 UNION ALL \
                 SELECT c.id FROM inventory_categories c \
                 JOIN category_tree parent ON c.parent_id = parent.id WHERE c.space_id = ? \
             ), location_tree AS ( \
                 SELECT id FROM inventory_locations WHERE id = ? AND space_id = ? \
                 UNION ALL \
                 SELECT l.id FROM inventory_locations l \
                 JOIN location_tree parent ON l.parent_id = parent.id WHERE l.space_id = ? \
             ) \
             SELECT id, space_id, inventory_number, name, description, historical_reference, technical_reference, state, revision, created_by_account_id, created_at \
             FROM inventory_items \
             WHERE space_id = ? AND inventory_number > ? \
               AND (? = '' OR LOCATE(?, name) > 0) \
               AND (? = 'all' OR state = ?) \
               AND (? IS NULL OR EXISTS ( \
                   SELECT 1 FROM item_category_assignments a \
                   JOIN category_tree t ON t.id = a.category_id \
                   WHERE a.item_id = inventory_items.id AND a.space_id = ? AND a.ended_at IS NULL \
               )) \
               AND (? IS NULL OR current_location_id IN (SELECT id FROM location_tree)) \
               AND (? IS NULL OR EXISTS ( \
                   SELECT 1 FROM inventory_group_members m \
                   WHERE m.item_id = inventory_items.id AND m.space_id = inventory_items.space_id \
                     AND m.group_id = ? \
               )) \
             ORDER BY inventory_number LIMIT ?",
        )
        .bind(filters.category_id.map(|id| id.to_string()))
        .bind(space_id.to_string())
        .bind(space_id.to_string())
        .bind(filters.location_id.map(|id| id.to_string()))
        .bind(filters.group_id.map(|id| id.to_string()))
        .bind(filters.group_id.map(|id| id.to_string()))
        .bind(space_id.to_string())
        .bind(space_id.to_string())
        .bind(space_id.to_string())
        .bind(after_inventory_number)
        .bind(search)
        .bind(search)
        .bind(state)
        .bind(state)
        .bind(filters.category_id.map(|id| id.to_string()))
        .bind(space_id.to_string())
        .bind(filters.location_id.map(|id| id.to_string()))
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(DatabaseError::Query)?;

        rows.iter().map(decode_item).collect()
    }

    /// Loads a space membership together with the account's explicit grants.
    ///
    /// Returns `Ok(None)` when the account is not a member. The caller must treat a missing
    /// membership as a refusal, never as an empty set of permissions that a default could fill.
    ///
    /// # Errors
    ///
    /// Returns [`DatabaseError`] when the query fails.
    pub async fn membership(
        &self,
        space_id: Uuid,
        account_id: Uuid,
    ) -> Result<Option<Membership>, DatabaseError> {
        let exists: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM space_memberships WHERE space_id = ? AND account_id = ?",
        )
        .bind(space_id.to_string())
        .bind(account_id.to_string())
        .fetch_one(&self.pool)
        .await
        .map_err(DatabaseError::Query)?;

        if exists == 0 {
            return Ok(None);
        }

        let rows = sqlx::query(
            "SELECT permission_code FROM space_permission_grants \
             WHERE space_id = ? AND account_id = ? ORDER BY permission_code",
        )
        .bind(space_id.to_string())
        .bind(account_id.to_string())
        .fetch_all(&self.pool)
        .await
        .map_err(DatabaseError::Query)?;

        // `permission_code` uses the ascii_bin collation, which MySQL reports as VARBINARY.
        let permissions = rows
            .iter()
            .map(|row| {
                let raw: Vec<u8> = row
                    .try_get("permission_code")
                    .map_err(DatabaseError::Query)?;
                let code = String::from_utf8(raw)
                    .map_err(|_| DatabaseError::Space(SpaceError::PermissionNotSpaceScoped))?;
                permission_from_code(&code)
                    .ok_or(DatabaseError::Space(SpaceError::PermissionNotSpaceScoped))
            })
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Some(Membership {
            space_id,
            account_id,
            permissions: permissions.into_iter().collect(),
        }))
    }

    /// Adds an account to a space with an explicit set of space-scoped grants.
    ///
    /// # Errors
    ///
    /// Returns [`DatabaseError::Space`] for an unknown account or space, an already existing
    /// membership, a grant that is not space-scoped, or an invalid name.
    pub async fn add_member(
        &self,
        space_id: Uuid,
        account_id: Uuid,
        permissions: &[Permission],
    ) -> Result<Membership, DatabaseError> {
        validate_grants(permissions)?;

        if self.space(space_id).await.is_err() {
            return Err(DatabaseError::Space(SpaceError::SpaceNotFound));
        }

        let account_exists: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM accounts WHERE id = ?")
            .bind(account_id.to_string())
            .fetch_one(&self.pool)
            .await
            .map_err(DatabaseError::Query)?;

        if account_exists == 0 {
            return Err(DatabaseError::Space(SpaceError::AccountNotFound));
        }

        let already_member: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM space_memberships WHERE space_id = ? AND account_id = ?",
        )
        .bind(space_id.to_string())
        .bind(account_id.to_string())
        .fetch_one(&self.pool)
        .await
        .map_err(DatabaseError::Query)?;

        if already_member > 0 {
            return Err(DatabaseError::Space(SpaceError::AlreadyMember));
        }

        let mut transaction = self.pool.begin().await.map_err(DatabaseError::Query)?;

        sqlx::query("INSERT INTO space_memberships (space_id, account_id) VALUES (?, ?)")
            .bind(space_id.to_string())
            .bind(account_id.to_string())
            .execute(&mut *transaction)
            .await
            .map_err(DatabaseError::Query)?;

        for permission in permissions {
            sqlx::query(
                "INSERT INTO space_permission_grants (space_id, account_id, permission_code) \
                 VALUES (?, ?, ?)",
            )
            .bind(space_id.to_string())
            .bind(account_id.to_string())
            .bind(permission_code(*permission))
            .execute(&mut *transaction)
            .await
            .map_err(DatabaseError::Query)?;
        }

        transaction.commit().await.map_err(DatabaseError::Query)?;

        self.membership(space_id, account_id)
            .await?
            .ok_or(DatabaseError::Space(SpaceError::NotMember))
    }

    /// Replaces the grants of an existing member.
    ///
    /// The actor cannot change its own grants, otherwise it could widen its own access. The
    /// owner keeps its membership and is only re-granted: removing the owner would leave a
    /// space with an owner who is not a member.
    ///
    /// # Errors
    ///
    /// Returns [`DatabaseError::Space`] for a missing membership, non-space-scoped grants,
    /// or an actor trying to change its own grants.
    pub async fn set_member_permissions(
        &self,
        space_id: Uuid,
        account_id: Uuid,
        permissions: &[Permission],
        actor_account_id: Uuid,
    ) -> Result<Membership, DatabaseError> {
        validate_grants(permissions)?;

        if account_id == actor_account_id {
            return Err(DatabaseError::Space(SpaceError::CannotChangeOwnGrants));
        }

        let mut transaction = self.pool.begin().await.map_err(DatabaseError::Query)?;
        require_member_manager(&mut transaction, space_id, actor_account_id).await?;
        let before = locked_member_grants(&mut transaction, space_id, account_id).await?;
        let selected: BTreeSet<Permission> = permissions.iter().copied().collect();
        let space = fetch_space(&mut *transaction, space_id).await?;
        if account_id == space.owner_account_id
            && (!selected.contains(&Permission::CollectionsRead)
                || !selected.contains(&Permission::CollectionsWrite))
        {
            return Err(DatabaseError::Space(SpaceError::OwnerCoreGrantsRequired));
        }

        sqlx::query("DELETE FROM space_permission_grants WHERE space_id = ? AND account_id = ?")
            .bind(space_id.to_string())
            .bind(account_id.to_string())
            .execute(&mut *transaction)
            .await
            .map_err(DatabaseError::Query)?;

        for permission in &selected {
            sqlx::query(
                "INSERT INTO space_permission_grants (space_id, account_id, permission_code) \
                 VALUES (?, ?, ?)",
            )
            .bind(space_id.to_string())
            .bind(account_id.to_string())
            .bind(permission_code(*permission))
            .execute(&mut *transaction)
            .await
            .map_err(DatabaseError::Query)?;
        }

        let mut after_codes = selected
            .into_iter()
            .map(permission_code)
            .collect::<Vec<_>>();
        after_codes.sort_unstable();
        let after = after_codes.join(",");
        insert_member_audit(
            &mut transaction,
            space_id,
            actor_account_id,
            account_id,
            "permissions_changed",
            &before,
            &after,
        )
        .await?;

        transaction.commit().await.map_err(DatabaseError::Query)?;

        self.membership(space_id, account_id)
            .await?
            .ok_or(DatabaseError::Space(SpaceError::NotMember))
    }

    /// Removes a member from a space, deleting its grants with the membership.
    ///
    /// The current owner cannot be removed: a space must always have exactly one owner, and
    /// ownership transfer is a separate, audited operation that does not exist yet.
    ///
    /// # Errors
    ///
    /// Returns [`DatabaseError::Space`] for a missing membership, an attempt to remove the
    /// owner, or an actor trying to change its own grants.
    pub async fn remove_member(
        &self,
        space_id: Uuid,
        account_id: Uuid,
        actor_account_id: Uuid,
    ) -> Result<bool, DatabaseError> {
        if account_id == actor_account_id {
            return Err(DatabaseError::Space(SpaceError::CannotChangeOwnGrants));
        }

        let mut transaction = self.pool.begin().await.map_err(DatabaseError::Query)?;
        require_member_manager(&mut transaction, space_id, actor_account_id).await?;
        let space = fetch_space(&mut *transaction, space_id).await?;
        if space.owner_account_id == account_id {
            return Err(DatabaseError::Space(SpaceError::OwnerCannotBeRemoved));
        }
        let before = locked_member_grants(&mut transaction, space_id, account_id).await?;

        sqlx::query("DELETE FROM space_permission_grants WHERE space_id = ? AND account_id = ?")
            .bind(space_id.to_string())
            .bind(account_id.to_string())
            .execute(&mut *transaction)
            .await
            .map_err(DatabaseError::Query)?;

        let removed =
            sqlx::query("DELETE FROM space_memberships WHERE space_id = ? AND account_id = ?")
                .bind(space_id.to_string())
                .bind(account_id.to_string())
                .execute(&mut *transaction)
                .await
                .map_err(DatabaseError::Query)?
                .rows_affected()
                == 1;

        insert_member_audit(
            &mut transaction,
            space_id,
            actor_account_id,
            account_id,
            "member_removed",
            &before,
            "",
        )
        .await?;

        transaction.commit().await.map_err(DatabaseError::Query)?;

        Ok(removed)
    }

    /// Lists the members of a space with their grants, for the sharing screen.
    ///
    /// # Errors
    ///
    /// Returns [`DatabaseError`] when the query fails.
    pub async fn space_members(&self, space_id: Uuid) -> Result<Vec<Membership>, DatabaseError> {
        let rows = sqlx::query(
            "SELECT account_id FROM space_memberships WHERE space_id = ? ORDER BY account_id",
        )
        .bind(space_id.to_string())
        .fetch_all(&self.pool)
        .await
        .map_err(DatabaseError::Query)?;

        let mut members = Vec::with_capacity(rows.len());
        for row in &rows {
            let account_id = parse_uuid(&decode_text(row, "account_id")?)?;
            if let Some(membership) = self.membership(space_id, account_id).await? {
                members.push(membership);
            }
        }

        Ok(members)
    }

    /// Lists members together with their account identity for the sharing screen.
    ///
    /// # Errors
    ///
    /// Returns a storage error when memberships or account identities cannot be loaded.
    pub async fn space_members_with_accounts(
        &self,
        space_id: Uuid,
    ) -> Result<Vec<MemberWithAccount>, DatabaseError> {
        let members = self.space_members(space_id).await?;
        let mut result = Vec::with_capacity(members.len());
        for membership in members {
            let row = sqlx::query("SELECT display_name, email FROM accounts WHERE id = ?")
                .bind(membership.account_id.to_string())
                .fetch_one(&self.pool)
                .await
                .map_err(DatabaseError::Query)?;
            result.push(MemberWithAccount {
                membership,
                display_name: row.try_get("display_name").map_err(DatabaseError::Query)?,
                email: row.try_get("email").map_err(DatabaseError::Query)?,
            });
        }
        Ok(result)
    }
}

/// Maximum length of a space name, aligned with the `VARCHAR(255)` column.
pub const MAX_SPACE_NAME_CHARS: usize = 255;

/// Maps a permission to the code stored in `space_permission_grants`.
///
/// The codes are the schema's `CHECK` list, so a value that is not space-scoped has no code
/// and must never reach the table.
pub(crate) const fn permission_code(permission: Permission) -> &'static str {
    match permission {
        Permission::CollectionsRead => "collections_read",
        Permission::CollectionsWrite => "collections_write",
        Permission::AcquisitionsRead => "acquisitions_read",
        Permission::AcquisitionsWrite => "acquisitions_write",
        Permission::FinanceRead => "finance_read",
        Permission::FinanceWrite => "finance_write",
        Permission::DocumentsRead => "documents_read",
        Permission::DocumentsWrite => "documents_write",
        // Non-space permissions have no stored code; callers are rejected before reaching here.
        _ => "",
    }
}

pub(crate) fn permission_from_code(code: &str) -> Option<Permission> {
    match code {
        "collections_read" => Some(Permission::CollectionsRead),
        "collections_write" => Some(Permission::CollectionsWrite),
        "acquisitions_read" => Some(Permission::AcquisitionsRead),
        "acquisitions_write" => Some(Permission::AcquisitionsWrite),
        "finance_read" => Some(Permission::FinanceRead),
        "finance_write" => Some(Permission::FinanceWrite),
        "documents_read" => Some(Permission::DocumentsRead),
        "documents_write" => Some(Permission::DocumentsWrite),
        _ => None,
    }
}

/// Parses a text identifier, reporting a missing row rather than a malformed UUID.
fn parse_uuid(value: &str) -> Result<Uuid, DatabaseError> {
    Uuid::parse_str(value).map_err(|_| DatabaseError::Space(SpaceError::SpaceNotFound))
}

fn validate_grants(permissions: &[Permission]) -> Result<(), DatabaseError> {
    for permission in permissions {
        if !permission.is_space_scoped() {
            return Err(DatabaseError::Space(SpaceError::PermissionNotSpaceScoped));
        }
    }

    Ok(())
}

async fn require_member_manager(
    transaction: &mut sqlx::Transaction<'_, sqlx::MySql>,
    space_id: Uuid,
    actor_id: Uuid,
) -> Result<(), DatabaseError> {
    let allowed: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM spaces s JOIN accounts a ON a.id = ? \
         WHERE s.id = ? AND (s.owner_account_id = a.id OR a.is_system_admin = TRUE)",
    )
    .bind(actor_id.to_string())
    .bind(space_id.to_string())
    .fetch_one(&mut **transaction)
    .await
    .map_err(DatabaseError::Query)?;
    if allowed == 0 {
        return Err(DatabaseError::Space(SpaceError::NotAuthorized));
    }
    Ok(())
}

async fn locked_member_grants(
    transaction: &mut sqlx::Transaction<'_, sqlx::MySql>,
    space_id: Uuid,
    account_id: Uuid,
) -> Result<String, DatabaseError> {
    let member: Option<Vec<u8>> = sqlx::query_scalar(
        "SELECT account_id FROM space_memberships WHERE space_id = ? AND account_id = ? FOR UPDATE",
    )
    .bind(space_id.to_string())
    .bind(account_id.to_string())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(DatabaseError::Query)?;
    if member.is_none() {
        return Err(DatabaseError::Space(SpaceError::NotMember));
    }
    let rows = sqlx::query(
        "SELECT permission_code FROM space_permission_grants WHERE space_id = ? AND account_id = ? ORDER BY permission_code",
    )
    .bind(space_id.to_string())
    .bind(account_id.to_string())
    .fetch_all(&mut **transaction)
    .await
    .map_err(DatabaseError::Query)?;
    rows.iter()
        .map(|row| decode_text(row, "permission_code"))
        .collect::<Result<Vec<_>, _>>()
        .map(|codes| codes.join(","))
}

async fn insert_member_audit(
    transaction: &mut sqlx::Transaction<'_, sqlx::MySql>,
    space_id: Uuid,
    actor_id: Uuid,
    target_id: Uuid,
    event_code: &str,
    before: &str,
    after: &str,
) -> Result<(), DatabaseError> {
    sqlx::query("INSERT INTO space_member_audit_events (id, space_id, actor_account_id, target_account_id, event_code, grants_before, grants_after, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)")
        .bind(Uuid::now_v7().to_string())
        .bind(space_id.to_string())
        .bind(actor_id.to_string())
        .bind(target_id.to_string())
        .bind(event_code)
        .bind(before)
        .bind(after)
        .bind(Utc::now())
        .execute(&mut **transaction)
        .await
        .map_err(DatabaseError::Query)?;
    Ok(())
}

/// The audit event code for a state transition is named after its destination.
fn state_event_code(target: ItemState) -> &'static str {
    match target {
        ItemState::Archived => "archived",
        ItemState::Trashed => "trashed",
        ItemState::Active => "restored",
    }
}

async fn insert_item_state_audit(
    transaction: &mut sqlx::Transaction<'_, sqlx::MySql>,
    item_id: Uuid,
    space_id: Uuid,
    actor_id: Uuid,
    before: ItemState,
    after: ItemState,
) -> Result<(), DatabaseError> {
    sqlx::query(
        "INSERT INTO inventory_item_audit_events \
             (id, item_id, space_id, actor_account_id, event_code, state_before, state_after, created_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(Uuid::now_v7().to_string())
    .bind(item_id.to_string())
    .bind(space_id.to_string())
    .bind(actor_id.to_string())
    .bind(state_event_code(after))
    .bind(before.as_str())
    .bind(after.as_str())
    .bind(Utc::now())
    .execute(&mut **transaction)
    .await
    .map_err(DatabaseError::Query)?;
    Ok(())
}

async fn fetch_space<'e, E>(executor: E, space_id: Uuid) -> Result<Space, DatabaseError>
where
    E: sqlx::Executor<'e, Database = sqlx::MySql>,
{
    let row = sqlx::query("SELECT id, name, owner_account_id, created_at FROM spaces WHERE id = ?")
        .bind(space_id.to_string())
        .fetch_optional(executor)
        .await
        .map_err(DatabaseError::Query)?
        .ok_or(DatabaseError::Space(SpaceError::SpaceNotFound))?;

    Ok(Space {
        id: parse_uuid(&decode_text(&row, "id")?)?,
        name: row.try_get("name").map_err(DatabaseError::Query)?,
        owner_account_id: parse_uuid(&decode_text(&row, "owner_account_id")?)?,
        created_at: row.try_get("created_at").map_err(DatabaseError::Query)?,
    })
}
/// Maximum length of an item name, aligned with the `VARCHAR(255)` column.
pub const MAX_ITEM_NAME_CHARS: usize = 255;

async fn fetch_item<'e, E>(executor: E, item_id: Uuid) -> Result<Item, DatabaseError>
where
    E: sqlx::Executor<'e, Database = sqlx::MySql>,
{
    let row = sqlx::query(
        "SELECT id, space_id, inventory_number, name, description, historical_reference, technical_reference, state, revision, created_by_account_id, created_at \
         FROM inventory_items WHERE id = ?",
    )
    .bind(item_id.to_string())
    .fetch_optional(executor)
    .await
    .map_err(DatabaseError::Query)?
    .ok_or(DatabaseError::Inventory(InventoryError::ItemNotFound))?;

    decode_item(&row)
}

fn decode_item(row: &sqlx::mysql::MySqlRow) -> Result<Item, DatabaseError> {
    Ok(Item {
        id: parse_uuid(&decode_text(row, "id")?)?,
        space_id: parse_uuid(&decode_text(row, "space_id")?)?,
        inventory_number: row
            .try_get("inventory_number")
            .map_err(DatabaseError::Query)?,
        name: row.try_get("name").map_err(DatabaseError::Query)?,
        description: row.try_get("description").map_err(DatabaseError::Query)?,
        historical_reference: row
            .try_get("historical_reference")
            .map_err(DatabaseError::Query)?,
        technical_reference: row
            .try_get("technical_reference")
            .map_err(DatabaseError::Query)?,
        state: decode_state(row)?,
        revision: row.try_get("revision").map_err(DatabaseError::Query)?,
        created_by_account_id: parse_uuid(&decode_text(row, "created_by_account_id")?)?,
        created_at: row.try_get("created_at").map_err(DatabaseError::Query)?,
    })
}

/// Decodes the `state` column, treating an unknown value as a storage inconsistency.
fn decode_state(row: &sqlx::mysql::MySqlRow) -> Result<ItemState, DatabaseError> {
    ItemState::from_code(&decode_text(row, "state")?)
        .ok_or(DatabaseError::Inventory(InventoryError::InvalidState))
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
        id: parse_uuid(&decode_text(row, "id")?)?,
        item_id: parse_uuid(&decode_text(row, "item_id")?)?,
        source_space_id: parse_uuid(&decode_text(row, "source_space_id")?)?,
        destination_space_id: parse_uuid(&decode_text(row, "destination_space_id")?)?,
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
    pub is_system_admin: bool,
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
    Space(SpaceError),
    Migration(sqlx::migrate::MigrateError),
    Query(sqlx::Error),
    Password(crate::credentials::PasswordError),
    TokenGeneration,
    InvalidCredentials,
    Session(SessionError),
}

impl DatabaseError {
    /// Returns the space rejection reason, when this error is one.
    #[must_use]
    pub const fn space_error(&self) -> Option<SpaceError> {
        match self {
            Self::Space(error) => Some(*error),
            _ => None,
        }
    }

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
            Self::Space(error) => write!(formatter, "space rejected: {error:?}"),
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
            | Self::Inventory(_)
            | Self::Space(_) => None,
        }
    }
}
