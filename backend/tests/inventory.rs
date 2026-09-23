//! Integration tests for inventory numbering and transfers.
//!
//! These exercise the real MariaDB locking behaviour, so they only run when
//! `COLLECTIONOPS_TEST_DATABASE_URL` is set. Each test takes a process-wide lock because the
//! suite shares one database and `sqlx::migrate!` is not concurrency-safe.

use collectionops_backend::{
    BootstrapAdmin, Database, InventoryError, ItemSearchFilters, ItemState, PasswordService,
};
use sqlx::Row;
use tokio::sync::{Mutex, MutexGuard};
use uuid::Uuid;

static DATABASE_LOCK: Mutex<()> = Mutex::const_new(());

const PASSWORD: &str = "correct horse battery staple";

/// A database with one account and two spaces, ready for inventory work.
struct Fixture {
    database: Database,
    owner_id: Uuid,
    /// First space, used as the source in transfer tests.
    space_a: Uuid,
    /// Second space, used as the destination in transfer tests.
    space_b: Uuid,
    _guard: MutexGuard<'static, ()>,
}

async fn fixture() -> Option<Fixture> {
    let database_url = std::env::var("COLLECTIONOPS_TEST_DATABASE_URL").ok()?;
    let guard = DATABASE_LOCK.lock().await;
    let database = Database::connect_and_migrate(&database_url)
        .await
        .expect("test database must accept the migrations");

    collectionops_backend::testing::clear_all(database.pool())
        .await
        .expect("the shared tables must be clearable");

    let admin = BootstrapAdmin::from_parts("owner@example.com", "Owner", PASSWORD).unwrap();
    database
        .provision_bootstrap_admin(&admin, &PasswordService::default())
        .await
        .expect("provisioning must succeed");

    let owner_id = read_uuid(
        &database,
        "SELECT id FROM accounts WHERE is_system_admin = TRUE",
    )
    .await;

    let space_a = create_space(&database, "Espace A", owner_id).await;
    let space_b = create_space(&database, "Espace B", owner_id).await;

    Some(Fixture {
        database,
        owner_id,
        space_a,
        space_b,
        _guard: guard,
    })
}

/// Inserts a raw space, to prove the inventory layer behaves even without `create_space`.
///
/// `space.id` and its counter are normally created together by
/// [`collectionops_backend::Database::create_space`], which other suites cover.
async fn create_space(database: &Database, name: &str, owner_id: Uuid) -> Uuid {
    let id = Uuid::now_v7();

    sqlx::query("INSERT INTO spaces (id, name, owner_account_id) VALUES (?, ?, ?)")
        .bind(id.to_string())
        .bind(name)
        .bind(owner_id.to_string())
        .execute(database.pool())
        .await
        .expect("the space must be insertable");

    sqlx::query("INSERT INTO inventory_counters (space_id, next_number) VALUES (?, 1)")
        .bind(id.to_string())
        .execute(database.pool())
        .await
        .expect("the counter must be insertable");

    id
}

async fn read_uuid(database: &Database, query: &str) -> Uuid {
    let raw: Vec<u8> = sqlx::query_scalar(query)
        .fetch_one(database.pool())
        .await
        .expect("the query must return one column");
    Uuid::parse_str(&String::from_utf8(raw).expect("the column must be UTF-8"))
        .expect("the column must hold a UUID")
}

#[tokio::test]
async fn items_get_consecutive_numbers_starting_at_one() {
    let Some(fixture) = fixture().await else {
        return;
    };

    let mut numbers = Vec::new();
    for name in ["Appareil photo", "Objectif", "Trépied"] {
        let item = fixture
            .database
            .create_item(fixture.space_a, name, fixture.owner_id)
            .await
            .expect("creation must succeed");

        assert_eq!(item.revision, 1, "a new item starts at revision 1");
        assert_eq!(item.name, name);
        numbers.push(item.inventory_number);
    }

    assert_eq!(
        numbers,
        vec![1, 2, 3],
        "numbers must be consecutive and start at 1"
    );
}

#[tokio::test]
async fn renaming_an_item_requires_the_current_revision() {
    let Some(fixture) = fixture().await else {
        return;
    };
    let item = fixture
        .database
        .create_item(fixture.space_a, "Ancien nom", fixture.owner_id)
        .await
        .unwrap();

    let renamed = fixture
        .database
        .rename_item(item.id, " Nouveau nom ", item.revision)
        .await
        .unwrap();
    assert_eq!(renamed.name, "Nouveau nom");
    assert_eq!(renamed.revision, item.revision + 1);
    assert_eq!(renamed.inventory_number, item.inventory_number);

    let stale = fixture
        .database
        .rename_item(item.id, "Ecrasement", item.revision)
        .await
        .unwrap_err();
    assert_eq!(
        stale.inventory_error(),
        Some(InventoryError::RevisionConflict)
    );
    assert_eq!(fixture.database.item(item.id).await.unwrap(), renamed);
}

#[tokio::test]
async fn each_space_has_its_own_numbering() {
    let Some(fixture) = fixture().await else {
        return;
    };

    let in_a = fixture
        .database
        .create_item(fixture.space_a, "Objet A", fixture.owner_id)
        .await
        .unwrap();
    let in_b = fixture
        .database
        .create_item(fixture.space_b, "Objet B", fixture.owner_id)
        .await
        .unwrap();

    assert_eq!(in_a.inventory_number, 1);
    assert_eq!(
        in_b.inventory_number, 1,
        "the second space starts its own sequence at 1"
    );
}

#[tokio::test]
async fn concurrent_creations_never_share_a_number() {
    let Some(fixture) = fixture().await else {
        return;
    };

    // Ten creations race for the same counter row. The `SELECT ... FOR UPDATE` lock must
    // serialize them, and the unique key on (space_id, inventory_number) is the backstop.
    let mut tasks = Vec::new();
    for index in 0..10 {
        let database = fixture.database.clone();
        let space_id = fixture.space_a;
        let owner_id = fixture.owner_id;
        tasks.push(tokio::spawn(async move {
            database
                .create_item(space_id, &format!("Objet {index}"), owner_id)
                .await
        }));
    }

    let mut numbers = Vec::new();
    for task in tasks {
        let item = task
            .await
            .expect("the task must not panic")
            .expect("concurrent creation must succeed");
        numbers.push(item.inventory_number);
    }

    numbers.sort_unstable();
    assert_eq!(
        numbers,
        (1..=10).collect::<Vec<u64>>(),
        "every concurrent creation must get a distinct number"
    );
}

#[tokio::test]
async fn a_failed_creation_does_not_burn_a_number() {
    let Some(fixture) = fixture().await else {
        return;
    };

    let first = fixture
        .database
        .create_item(fixture.space_a, "Premier", fixture.owner_id)
        .await
        .unwrap();
    assert_eq!(first.inventory_number, 1);

    // An empty name is rejected before the transaction, so nothing is reserved.
    assert_eq!(
        fixture
            .database
            .create_item(fixture.space_a, "   ", fixture.owner_id)
            .await
            .err()
            .and_then(|error| error.inventory_error()),
        Some(InventoryError::InvalidName)
    );

    let next = fixture
        .database
        .create_item(fixture.space_a, "Deuxième", fixture.owner_id)
        .await
        .unwrap();
    assert_eq!(
        next.inventory_number, 2,
        "the refused attempt burned no number"
    );
}

#[tokio::test]
async fn a_name_is_trimmed_and_bounded() {
    let Some(fixture) = fixture().await else {
        return;
    };

    let item = fixture
        .database
        .create_item(fixture.space_a, "  Appareil photo  ", fixture.owner_id)
        .await
        .unwrap();
    assert_eq!(item.name, "Appareil photo");

    assert_eq!(
        fixture
            .database
            .create_item(fixture.space_a, &"a".repeat(256), fixture.owner_id)
            .await
            .err()
            .and_then(|error| error.inventory_error()),
        Some(InventoryError::InvalidName)
    );

    // Exactly at the limit is accepted.
    let at_limit = fixture
        .database
        .create_item(fixture.space_a, &"b".repeat(255), fixture.owner_id)
        .await
        .unwrap();
    assert_eq!(at_limit.name.len(), 255);
}

#[tokio::test]
async fn a_transfer_renumbers_the_item_and_keeps_the_old_number_in_history() {
    let Some(fixture) = fixture().await else {
        return;
    };

    // Space B already holds two objects, so the moved item must land on number 3.
    fixture
        .database
        .create_item(fixture.space_b, "B1", fixture.owner_id)
        .await
        .unwrap();
    fixture
        .database
        .create_item(fixture.space_b, "B2", fixture.owner_id)
        .await
        .unwrap();

    let item = fixture
        .database
        .create_item(fixture.space_a, "Appareil photo", fixture.owner_id)
        .await
        .unwrap();
    assert_eq!(item.inventory_number, 1);

    let outcome = fixture
        .database
        .transfer_item(item.id, fixture.space_b, 1, fixture.owner_id)
        .await
        .expect("the transfer must succeed");

    assert_eq!(outcome.item.id, item.id, "the identifier never changes");
    assert_eq!(outcome.item.space_id, fixture.space_b);
    assert_eq!(outcome.item.inventory_number, 3, "renumbered on arrival");
    assert_eq!(outcome.item.revision, 2, "a transfer is a new revision");

    assert_eq!(outcome.transfer.source_space_id, fixture.space_a);
    assert_eq!(
        outcome.transfer.source_inventory_number, 1,
        "the old number is kept"
    );
    assert_eq!(outcome.transfer.destination_space_id, fixture.space_b);
    assert_eq!(outcome.transfer.destination_inventory_number, 3);

    let history = fixture.database.item_transfers(item.id).await.unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].id, outcome.transfer.id);

    // The old number in the source space is free again but must not be reused by the item.
    let back = fixture
        .database
        .transfer_item(item.id, fixture.space_a, 2, fixture.owner_id)
        .await
        .unwrap();
    assert_eq!(
        back.item.inventory_number, 2,
        "the source counter moved on, so the item does not reclaim number 1"
    );
    assert_eq!(back.transfer.source_inventory_number, 3);
}

#[tokio::test]
async fn a_stale_revision_is_refused_and_changes_nothing() {
    let Some(fixture) = fixture().await else {
        return;
    };

    let item = fixture
        .database
        .create_item(fixture.space_a, "Objet", fixture.owner_id)
        .await
        .unwrap();

    let moved = fixture
        .database
        .transfer_item(item.id, fixture.space_b, 1, fixture.owner_id)
        .await
        .unwrap();
    assert_eq!(moved.item.revision, 2);

    // A second client still believes the item is at revision 1 and is on space A.
    assert_eq!(
        fixture
            .database
            .transfer_item(item.id, fixture.space_a, 1, fixture.owner_id)
            .await
            .err()
            .and_then(|error| error.inventory_error()),
        Some(InventoryError::RevisionConflict)
    );

    // Nothing was lost: the item is still where the accepted transfer put it.
    let current = fixture.database.item(item.id).await.unwrap();
    assert_eq!(current.space_id, fixture.space_b);
    assert_eq!(current.revision, 2);
    assert_eq!(
        fixture
            .database
            .item_transfers(item.id)
            .await
            .unwrap()
            .len(),
        1,
        "the refused transfer must leave no history entry"
    );

    // The refused transfer consumed no number: only the accepted move advanced space B's
    // counter, so the next arrival takes number 2, not 3.
    let other = fixture
        .database
        .create_item(fixture.space_a, "Autre", fixture.owner_id)
        .await
        .unwrap();
    let moved_other = fixture
        .database
        .transfer_item(other.id, fixture.space_b, 1, fixture.owner_id)
        .await
        .unwrap();
    assert_eq!(
        moved_other.item.inventory_number, 2,
        "the refused transfer must not have reserved a destination number"
    );
}

#[tokio::test]
async fn transferring_into_the_current_space_is_refused() {
    let Some(fixture) = fixture().await else {
        return;
    };

    let item = fixture
        .database
        .create_item(fixture.space_a, "Objet", fixture.owner_id)
        .await
        .unwrap();

    assert_eq!(
        fixture
            .database
            .transfer_item(item.id, fixture.space_a, 1, fixture.owner_id)
            .await
            .err()
            .and_then(|error| error.inventory_error()),
        Some(InventoryError::SameSpace)
    );
}

#[tokio::test]
async fn unknown_items_and_spaces_are_reported_as_not_found() {
    let Some(fixture) = fixture().await else {
        return;
    };

    let missing = Uuid::now_v7();

    assert_eq!(
        fixture
            .database
            .item(missing)
            .await
            .err()
            .and_then(|error| error.inventory_error()),
        Some(InventoryError::ItemNotFound)
    );
    assert_eq!(
        fixture
            .database
            .space_owner(missing)
            .await
            .err()
            .and_then(|error| error.inventory_error()),
        Some(InventoryError::SpaceNotFound)
    );

    let item = fixture
        .database
        .create_item(fixture.space_a, "Objet", fixture.owner_id)
        .await
        .unwrap();
    assert_eq!(
        fixture
            .database
            .transfer_item(item.id, missing, 1, fixture.owner_id)
            .await
            .err()
            .and_then(|error| error.inventory_error()),
        Some(InventoryError::CounterMissing),
        "a destination with no counter row cannot accept an item"
    );
}

#[tokio::test]
async fn concurrent_transfers_of_the_same_item_are_serialized() {
    let Some(fixture) = fixture().await else {
        return;
    };

    let item = fixture
        .database
        .create_item(fixture.space_a, "Objet", fixture.owner_id)
        .await
        .unwrap();

    // Two devices move the same item at revision 1, to different spaces. The row lock must
    // let exactly one win, and the loser must be told its revision is stale.
    let mut tasks = Vec::new();
    for destination in [fixture.space_a, fixture.space_b] {
        let database = fixture.database.clone();
        let item_id = item.id;
        let owner_id = fixture.owner_id;
        tasks.push(tokio::spawn(async move {
            database
                .transfer_item(item_id, destination, 1, owner_id)
                .await
        }));
    }

    let mut accepted = 0;
    let mut conflicted = 0;
    for task in tasks {
        match task.await.expect("the task must not panic") {
            Ok(_) => accepted += 1,
            Err(error) => {
                assert!(
                    matches!(
                        error.inventory_error(),
                        Some(InventoryError::RevisionConflict | InventoryError::SameSpace)
                    ),
                    "the loser must fail on the revision or the destination, got {error:?}"
                );
                conflicted += 1;
            }
        }
    }

    assert_eq!(accepted, 1, "exactly one transfer may win");
    assert_eq!(conflicted, 1);

    let current = fixture.database.item(item.id).await.unwrap();
    assert_eq!(current.revision, 2, "the item was moved exactly once");
    assert_eq!(
        fixture
            .database
            .item_transfers(item.id)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn archiving_and_restoring_cycle_through_the_states() {
    let Some(fixture) = fixture().await else {
        return;
    };

    let item = fixture
        .database
        .create_item(fixture.space_a, "Objet", fixture.owner_id)
        .await
        .unwrap();
    assert_eq!(item.state, ItemState::Active, "a new item starts active");

    let archived = fixture
        .database
        .set_item_state(
            item.id,
            ItemState::Archived,
            item.revision,
            fixture.owner_id,
        )
        .await
        .unwrap();
    assert_eq!(archived.state, ItemState::Archived);
    assert_eq!(archived.revision, item.revision + 1);
    assert_eq!(
        archived.inventory_number, item.inventory_number,
        "the number never changes"
    );

    let restored = fixture
        .database
        .set_item_state(
            item.id,
            ItemState::Active,
            archived.revision,
            fixture.owner_id,
        )
        .await
        .unwrap();
    assert_eq!(restored.state, ItemState::Active);

    let trashed = fixture
        .database
        .set_item_state(
            item.id,
            ItemState::Trashed,
            restored.revision,
            fixture.owner_id,
        )
        .await
        .unwrap();
    assert_eq!(trashed.state, ItemState::Trashed);

    let restored_again = fixture
        .database
        .set_item_state(
            item.id,
            ItemState::Active,
            trashed.revision,
            fixture.owner_id,
        )
        .await
        .unwrap();
    assert_eq!(restored_again.state, ItemState::Active);
    assert_eq!(
        restored_again.id, item.id,
        "the stable identifier is preserved across every transition"
    );
}

#[tokio::test]
async fn an_invalid_transition_is_refused_and_changes_nothing() {
    let Some(fixture) = fixture().await else {
        return;
    };

    let item = fixture
        .database
        .create_item(fixture.space_a, "Objet", fixture.owner_id)
        .await
        .unwrap();

    // An active item cannot be archived into the state it already has.
    assert_eq!(
        fixture
            .database
            .set_item_state(item.id, ItemState::Active, item.revision, fixture.owner_id)
            .await
            .err()
            .and_then(|error| error.inventory_error()),
        Some(InventoryError::InvalidTransition)
    );

    let current = fixture.database.item(item.id).await.unwrap();
    assert_eq!(current.state, ItemState::Active);
    assert_eq!(current.revision, item.revision);
}

#[tokio::test]
async fn a_stale_state_revision_is_refused() {
    let Some(fixture) = fixture().await else {
        return;
    };

    let item = fixture
        .database
        .create_item(fixture.space_a, "Objet", fixture.owner_id)
        .await
        .unwrap();
    fixture
        .database
        .set_item_state(
            item.id,
            ItemState::Archived,
            item.revision,
            fixture.owner_id,
        )
        .await
        .unwrap();

    assert_eq!(
        fixture
            .database
            .set_item_state(item.id, ItemState::Trashed, item.revision, fixture.owner_id)
            .await
            .err()
            .and_then(|error| error.inventory_error()),
        Some(InventoryError::RevisionConflict)
    );

    let current = fixture.database.item(item.id).await.unwrap();
    assert_eq!(current.state, ItemState::Archived);
    assert_eq!(current.revision, item.revision + 1);
}

#[tokio::test]
async fn a_trashed_item_cannot_be_renamed_or_transferred() {
    let Some(fixture) = fixture().await else {
        return;
    };

    let item = fixture
        .database
        .create_item(fixture.space_a, "Objet", fixture.owner_id)
        .await
        .unwrap();
    let trashed = fixture
        .database
        .set_item_state(item.id, ItemState::Trashed, item.revision, fixture.owner_id)
        .await
        .unwrap();

    assert_eq!(
        fixture
            .database
            .rename_item(item.id, "Nouveau", trashed.revision)
            .await
            .err()
            .and_then(|error| error.inventory_error()),
        Some(InventoryError::InvalidState)
    );
    assert_eq!(
        fixture
            .database
            .transfer_item(item.id, fixture.space_b, trashed.revision, fixture.owner_id)
            .await
            .err()
            .and_then(|error| error.inventory_error()),
        Some(InventoryError::InvalidState)
    );

    let restored = fixture
        .database
        .set_item_state(
            item.id,
            ItemState::Active,
            trashed.revision,
            fixture.owner_id,
        )
        .await
        .unwrap();
    assert_eq!(restored.state, ItemState::Active);
}

#[tokio::test]
async fn the_state_filter_hides_archived_and_trashed_items() {
    let Some(fixture) = fixture().await else {
        return;
    };

    let active = fixture
        .database
        .create_item(fixture.space_a, "Actif", fixture.owner_id)
        .await
        .unwrap();
    let archived = fixture
        .database
        .create_item(fixture.space_a, "Archive", fixture.owner_id)
        .await
        .unwrap();
    let trashed = fixture
        .database
        .create_item(fixture.space_a, "Corbeille", fixture.owner_id)
        .await
        .unwrap();
    fixture
        .database
        .set_item_state(
            archived.id,
            ItemState::Archived,
            archived.revision,
            fixture.owner_id,
        )
        .await
        .unwrap();
    fixture
        .database
        .set_item_state(
            trashed.id,
            ItemState::Trashed,
            trashed.revision,
            fixture.owner_id,
        )
        .await
        .unwrap();

    let visible = fixture
        .database
        .search_items_in_space(
            fixture.space_a,
            "",
            0,
            "active",
            50,
            ItemSearchFilters::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        visible.iter().map(|item| item.id).collect::<Vec<_>>(),
        vec![active.id],
        "the default view only shows active items"
    );

    let all = fixture
        .database
        .search_items_in_space(
            fixture.space_a,
            "",
            0,
            "all",
            50,
            ItemSearchFilters::default(),
        )
        .await
        .unwrap();
    assert_eq!(all.len(), 3);

    let archived_only = fixture
        .database
        .search_items_in_space(
            fixture.space_a,
            "",
            0,
            "archived",
            50,
            ItemSearchFilters::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        archived_only.iter().map(|item| item.id).collect::<Vec<_>>(),
        vec![archived.id]
    );
}

#[tokio::test]
async fn every_state_transition_is_audited_with_actor_and_states() {
    let Some(fixture) = fixture().await else {
        return;
    };

    let item = fixture
        .database
        .create_item(fixture.space_a, "Objet", fixture.owner_id)
        .await
        .unwrap();
    let archived = fixture
        .database
        .set_item_state(
            item.id,
            ItemState::Archived,
            item.revision,
            fixture.owner_id,
        )
        .await
        .unwrap();
    fixture
        .database
        .set_item_state(
            item.id,
            ItemState::Active,
            archived.revision,
            fixture.owner_id,
        )
        .await
        .unwrap();

    let rows = sqlx::query(
        "SELECT event_code, state_before, state_after, actor_account_id \
         FROM inventory_item_audit_events WHERE item_id = ? ORDER BY created_at, id",
    )
    .bind(item.id.to_string())
    .fetch_all(fixture.database.pool())
    .await
    .unwrap();

    assert_eq!(rows.len(), 2, "each transition appends one event");
    let code: Vec<u8> = rows[0].try_get("event_code").unwrap();
    let before: Vec<u8> = rows[0].try_get("state_before").unwrap();
    let after: Vec<u8> = rows[0].try_get("state_after").unwrap();
    let actor: Vec<u8> = rows[0].try_get("actor_account_id").unwrap();
    assert_eq!(String::from_utf8(code).unwrap(), "archived");
    assert_eq!(String::from_utf8(before).unwrap(), "active");
    assert_eq!(String::from_utf8(after).unwrap(), "archived");
    assert_eq!(
        String::from_utf8(actor).unwrap(),
        fixture.owner_id.to_string()
    );
}
