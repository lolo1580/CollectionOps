//! Integration tests for per-device sessions.
//!
//! They only run when `COLLECTIONOPS_TEST_DATABASE_URL` is set. The tests share one database
//! and `sqlx::migrate!` is not safe to run concurrently, so each takes a process-wide lock.

use collectionops_backend::{
    BootstrapAdmin, Database, IdleTimeout, PasswordService, SessionError, SessionLifetime,
};
use tokio::sync::{Mutex, MutexGuard};

static DATABASE_LOCK: Mutex<()> = Mutex::const_new(());

const PASSWORD: &str = "correct horse battery staple";

async fn test_database() -> Option<(Database, MutexGuard<'static, ()>)> {
    let database_url = std::env::var("COLLECTIONOPS_TEST_DATABASE_URL").ok()?;
    let guard = DATABASE_LOCK.lock().await;
    let database = Database::connect_and_migrate(&database_url)
        .await
        .expect("test database must accept the migrations");

    Some((database, guard))
}

/// Creates the administrator and returns the database plus a service for hashing.
async fn database_with_admin() -> Option<(Database, MutexGuard<'static, ()>, PasswordService)> {
    let (database, guard) = test_database().await?;

    collectionops_backend::testing::clear_all(database.pool())
        .await
        .expect("the shared tables must be clearable");

    let admin = BootstrapAdmin::from_parts("root@example.com", "Root", PASSWORD)
        .expect("the test administrator must be valid");
    let passwords = PasswordService::default();
    database
        .provision_bootstrap_admin(&admin, &passwords)
        .await
        .expect("provisioning must succeed");

    Some((database, guard, passwords))
}

#[tokio::test]
async fn every_login_issues_a_new_token_for_the_device() {
    let Some((database, _guard, passwords)) = database_with_admin().await else {
        return;
    };
    let email = collectionops_backend::EmailAddress::parse("root@example.com").unwrap();

    let first = database
        .create_session(
            &email,
            PASSWORD,
            Some("Bureau"),
            IdleTimeout::Minutes30,
            false,
            &passwords,
        )
        .await
        .expect("the first login must succeed");
    let second = database
        .create_session(
            &email,
            PASSWORD,
            Some("Portable"),
            IdleTimeout::Hour1,
            false,
            &passwords,
        )
        .await
        .expect("the second login must succeed");

    assert_ne!(
        first.token().expose_secret(),
        second.token().expose_secret(),
        "each login must issue a brand new token"
    );
    assert_ne!(first.session.id, second.session.id);
    assert_eq!(first.session.device_label.as_deref(), Some("Bureau"));
    assert_eq!(second.session.device_label.as_deref(), Some("Portable"));

    // Both devices stay usable independently: one login never invalidates the other.
    assert!(database.authenticate_session(first.token()).await.is_ok());
    assert!(database.authenticate_session(second.token()).await.is_ok());
}

#[tokio::test]
async fn a_wrong_password_never_creates_a_session() {
    let Some((database, _guard, passwords)) = database_with_admin().await else {
        return;
    };
    let email = collectionops_backend::EmailAddress::parse("root@example.com").unwrap();

    assert!(
        database
            .create_session(
                &email,
                "not the password",
                None,
                IdleTimeout::Minutes30,
                false,
                &passwords
            )
            .await
            .is_err()
    );
    assert!(
        database
            .create_session(
                &collectionops_backend::EmailAddress::parse("nobody@example.com").unwrap(),
                PASSWORD,
                None,
                IdleTimeout::Minutes30,
                false,
                &passwords
            )
            .await
            .is_err()
    );

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM account_sessions")
        .fetch_one(database.pool())
        .await
        .unwrap();
    assert_eq!(count, 0, "a failed login must not leave a session behind");
}

#[tokio::test]
async fn the_token_lifetime_is_capped_at_seven_days() {
    let Some((database, _guard, passwords)) = database_with_admin().await else {
        return;
    };
    let email = collectionops_backend::EmailAddress::parse("root@example.com").unwrap();

    let standard = database
        .create_session(
            &email,
            PASSWORD,
            None,
            IdleTimeout::Minutes15,
            false,
            &passwords,
        )
        .await
        .unwrap();
    let persistent = database
        .create_session(&email, PASSWORD, None, IdleTimeout::Never, true, &passwords)
        .await
        .unwrap();

    let standard_lifetime = standard.session.absolute_expires_at - standard.session.created_at;
    let persistent_lifetime =
        persistent.session.absolute_expires_at - persistent.session.created_at;

    assert_eq!(
        standard_lifetime.num_seconds(),
        i64::from(SessionLifetime::DEFAULT_SECONDS)
    );
    assert_eq!(
        persistent_lifetime.num_seconds(),
        i64::from(SessionLifetime::MAX_SECONDS)
    );
    assert!(
        persistent_lifetime.num_seconds() <= 7 * 24 * 60 * 60,
        "even a persistent session must not outlive seven days"
    );
}

#[tokio::test]
async fn a_revoked_token_stops_working_immediately() {
    let Some((database, _guard, passwords)) = database_with_admin().await else {
        return;
    };
    let email = collectionops_backend::EmailAddress::parse("root@example.com").unwrap();
    let issued = database
        .create_session(
            &email,
            PASSWORD,
            Some("Vole"),
            IdleTimeout::Never,
            true,
            &passwords,
        )
        .await
        .unwrap();

    assert!(database.authenticate_session(issued.token()).await.is_ok());

    assert!(
        database.revoke_session(&issued.session.id).await.unwrap(),
        "the first revocation must report a change"
    );
    assert!(
        !database.revoke_session(&issued.session.id).await.unwrap(),
        "revoking twice must not report a second change"
    );

    assert_eq!(
        database
            .authenticate_session(issued.token())
            .await
            .err()
            .and_then(|error| error.session_error()),
        Some(SessionError::Expired),
        "a revoked token must be refused"
    );
}

#[tokio::test]
async fn an_unknown_token_is_refused() {
    let Some((database, _guard, _passwords)) = database_with_admin().await else {
        return;
    };

    let forged = collectionops_backend::SessionToken::generate().unwrap();

    assert_eq!(
        database
            .authenticate_session(&forged)
            .await
            .err()
            .and_then(|error| error.session_error()),
        Some(SessionError::Unknown)
    );
}

#[tokio::test]
async fn an_idle_session_expires_but_activity_keeps_it_alive() {
    let Some((database, _guard, passwords)) = database_with_admin().await else {
        return;
    };
    let email = collectionops_backend::EmailAddress::parse("root@example.com").unwrap();
    let issued = database
        .create_session(
            &email,
            PASSWORD,
            None,
            IdleTimeout::Minutes15,
            false,
            &passwords,
        )
        .await
        .unwrap();

    // Normal use refreshes the idle clock.
    let authenticated = database.authenticate_session(issued.token()).await.unwrap();
    assert_eq!(authenticated.display_name, "Root");

    // Age the session beyond its idle delay while keeping the row self-consistent: the
    // schema forbids `last_seen_at < created_at`, so the whole session is backdated.
    sqlx::query(
        "UPDATE account_sessions \
         SET created_at = DATE_SUB(CURRENT_TIMESTAMP(6), INTERVAL 20 MINUTE), \
             last_seen_at = DATE_SUB(CURRENT_TIMESTAMP(6), INTERVAL 16 MINUTE), \
             absolute_expires_at = DATE_ADD(CURRENT_TIMESTAMP(6), INTERVAL 11 HOUR) \
         WHERE id = ?",
    )
    .bind(&issued.session.id)
    .execute(database.pool())
    .await
    .unwrap();

    assert_eq!(
        database
            .authenticate_session(issued.token())
            .await
            .err()
            .and_then(|error| error.session_error()),
        Some(SessionError::Expired),
        "an idle session must be refused"
    );
}

#[tokio::test]
async fn the_session_list_exposes_no_secret_and_hides_the_token() {
    let Some((database, _guard, passwords)) = database_with_admin().await else {
        return;
    };
    let email = collectionops_backend::EmailAddress::parse("root@example.com").unwrap();
    let issued = database
        .create_session(
            &email,
            PASSWORD,
            Some("Bureau"),
            IdleTimeout::Hour1,
            false,
            &passwords,
        )
        .await
        .unwrap();

    let account_id: Vec<u8> =
        sqlx::query_scalar("SELECT id FROM accounts WHERE is_system_admin = TRUE")
            .fetch_one(database.pool())
            .await
            .unwrap();
    let account_id = String::from_utf8(account_id).unwrap();

    let sessions = database.list_sessions(&account_id).await.unwrap();

    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, issued.session.id);
    assert_eq!(sessions[0].idle_timeout, IdleTimeout::Hour1);
    assert_eq!(sessions[0].device_label.as_deref(), Some("Bureau"));
    assert!(sessions[0].revoked_at.is_none());
}
