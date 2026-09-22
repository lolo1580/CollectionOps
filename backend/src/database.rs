use std::{error::Error, fmt};

use sqlx::{MySqlPool, mysql::MySqlPoolOptions};

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
}

#[derive(Debug)]
pub enum DatabaseError {
    Connection(sqlx::Error),
    Migration(sqlx::migrate::MigrateError),
}

impl fmt::Display for DatabaseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connection(error) => write!(formatter, "MariaDB connection failed: {error}"),
            Self::Migration(error) => write!(formatter, "MariaDB migration failed: {error}"),
        }
    }
}

impl Error for DatabaseError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Connection(error) => Some(error),
            Self::Migration(error) => Some(error),
        }
    }
}
