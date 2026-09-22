use collectionops_backend::Database;

#[tokio::test]
async fn embedded_migration_creates_the_core_tables() {
    let Ok(database_url) = std::env::var("COLLECTIONOPS_TEST_DATABASE_URL") else {
        return;
    };

    let database = Database::connect_and_migrate(&database_url)
        .await
        .expect("test database must accept the initial migration");

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
