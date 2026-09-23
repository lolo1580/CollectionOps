//! Test helpers shared by the integration suites.
//!
//! The integration tests run against one shared database, so they all need the same
//! delete order: children before parents. Keeping it in one place means adding a table
//! cannot silently break an unrelated suite with a foreign-key error.

/// Deletes every row the test suites own, in foreign-key order.
///
/// Order matters: transfers reference items and spaces, items reference spaces and accounts,
/// counters reference spaces, and sessions reference accounts.
///
/// # Errors
///
/// Returns the underlying [`sqlx::Error`] when a statement fails.
pub async fn clear_all(pool: &sqlx::MySqlPool) -> Result<(), sqlx::Error> {
    for statement in [
        "DELETE FROM space_member_audit_events",
        "DELETE FROM space_audit_events",
        "DELETE FROM inventory_item_audit_events",
        "DELETE FROM space_invitation_grants",
        "DELETE FROM space_invitations",
        "DELETE FROM inventory_transfers",
        "DELETE FROM inventory_items",
        "DELETE FROM inventory_counters",
        "DELETE FROM space_permission_grants",
        "DELETE FROM space_memberships",
        "DELETE FROM spaces",
        "DELETE FROM account_sessions",
        "DELETE FROM accounts",
    ] {
        sqlx::query(statement).execute(pool).await?;
    }

    Ok(())
}
