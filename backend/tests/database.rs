//! Integration tests for the MariaDB core.
//!
//! They only run when `COLLECTIONOPS_TEST_DATABASE_URL` is set, so a checkout without a
//! database still passes `cargo test`. The tests share one database, and `sqlx::migrate!`
//! is not safe to run concurrently, so each test takes a process-wide lock first.

use collectionops_backend::{BootstrapAdmin, Database, PasswordService};
use sqlx::Row;
use tokio::sync::{Mutex, MutexGuard};

static DATABASE_LOCK: Mutex<()> = Mutex::const_new(());

/// Connects to the test database, applies the migrations, and holds the test lock.
///
/// The guard is returned to the caller so the lock lives exactly as long as the test does.
async fn test_database() -> Option<(Database, MutexGuard<'static, ()>)> {
    let database_url = std::env::var("COLLECTIONOPS_TEST_DATABASE_URL").ok()?;
    let guard = DATABASE_LOCK.lock().await;
    let database = Database::connect_and_migrate(&database_url)
        .await
        .expect("test database must accept the migrations");

    Some((database, guard))
}

#[tokio::test]
async fn embedded_migration_creates_the_core_tables() {
    let Some((database, _guard)) = test_database().await else {
        return;
    };

    let table_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM information_schema.tables \
         WHERE table_schema = DATABASE() \
         AND table_name IN (\
             'accounts', 'spaces', 'space_memberships', 'space_permission_grants', \
             'inventory_counters', 'inventory_items', 'inventory_transfers'\
         )",
    )
    .fetch_one(database.pool())
    .await
    .expect("schema metadata must be readable");

    assert_eq!(table_count, 7);
}

#[tokio::test]
async fn migrations_add_the_credential_columns_and_never_a_plaintext_password() {
    let Some((database, _guard)) = test_database().await else {
        return;
    };

    let columns: Vec<String> = sqlx::query(
        "SELECT column_name FROM information_schema.columns \
         WHERE table_schema = DATABASE() AND table_name = 'accounts' \
         AND column_name IN ('email', 'password_hash', 'email_verified_at')",
    )
    .fetch_all(database.pool())
    .await
    .expect("column metadata must be readable")
    .into_iter()
    .map(|row| row.get::<String, _>("column_name"))
    .collect();

    assert_eq!(
        columns.len(),
        3,
        "the credential columns must exist, found: {columns:?}"
    );

    let plaintext_columns: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM information_schema.columns \
         WHERE table_schema = DATABASE() \
         AND column_name IN ('password', 'password_plain', 'plaintext_password')",
    )
    .fetch_one(database.pool())
    .await
    .expect("column metadata must be readable");

    assert_eq!(
        plaintext_columns, 0,
        "no column may be reserved for a plaintext password"
    );
}

#[tokio::test]
async fn provisioning_creates_exactly_one_administrator_and_is_idempotent() {
    let Some((database, _guard)) = test_database().await else {
        return;
    };

    // Keep the assertions independent from whatever the shared test database already holds.
    sqlx::query("DELETE FROM accounts WHERE is_system_admin = TRUE")
        .execute(database.pool())
        .await
        .expect("the test database must be writable");

    let admin =
        BootstrapAdmin::from_parts("Root@Example.com", "Root", "correct horse battery staple")
            .expect("the test administrator must be valid");
    let passwords = PasswordService::default();

    let created = database
        .provision_bootstrap_admin(&admin, &passwords)
        .await
        .expect("provisioning must succeed");
    assert!(created, "the first call must create the administrator");
    assert_eq!(database.system_admin_count().await.unwrap(), 1);

    let created_again = database
        .provision_bootstrap_admin(&admin, &passwords)
        .await
        .expect("a second provisioning must not fail");
    assert!(!created_again, "the second call must be a no-op");
    assert_eq!(database.system_admin_count().await.unwrap(), 1);

    // `password_hash` uses the ascii_bin collation, which MySQL reports as VARBINARY, so the
    // value is read as bytes and converted explicitly instead of relying on a String mapping.
    let stored = sqlx::query(
        "SELECT display_name, email, password_hash FROM accounts WHERE is_system_admin = TRUE",
    )
    .fetch_one(database.pool())
    .await
    .expect("the administrator must be readable");

    let display_name: String = stored.get("display_name");
    let email: String = stored.get("email");
    let hash: Vec<u8> = stored.get("password_hash");
    let hash = String::from_utf8(hash).expect("the PHC hash must be valid UTF-8");

    assert_eq!(display_name, "Root");
    assert_eq!(email, "Root@example.com", "the domain must be normalised");
    assert!(
        hash.starts_with("$argon2id$v=19$"),
        "the stored value must be an Argon2id PHC hash, got {hash:?}"
    );
    assert!(
        !hash.contains("correct horse battery staple"),
        "the plaintext password must never reach the database"
    );
}
