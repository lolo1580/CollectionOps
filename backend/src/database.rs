use std::{error::Error, fmt};

use sqlx::{MySqlPool, mysql::MySqlPoolOptions};
use uuid::Uuid;

use crate::{config::BootstrapAdmin, credentials::PasswordService};

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
}

#[derive(Debug)]
pub enum DatabaseError {
    Connection(sqlx::Error),
    Migration(sqlx::migrate::MigrateError),
    Query(sqlx::Error),
    Password(crate::credentials::PasswordError),
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
        }
    }
}

impl Error for DatabaseError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Connection(error) | Self::Query(error) => Some(error),
            Self::Migration(error) => Some(error),
            Self::Password(error) => Some(error),
        }
    }
}
