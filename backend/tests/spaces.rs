//! Integration tests for spaces, memberships and the space authorization boundary.
//!
//! These are the first tests that exercise `require_space_permission` end to end: the
//! membership is loaded from storage and handed to the authorization check, which is what
//! ADR-0004 requires from every future space-scoped route.

use collectionops_backend::{
    BootstrapAdmin, Database, PasswordService, Permission, Principal, SpaceError,
};
use std::collections::BTreeSet;
use tokio::sync::{Mutex, MutexGuard};
use uuid::Uuid;

static DATABASE_LOCK: Mutex<()> = Mutex::const_new(());

const PASSWORD: &str = "correct horse battery staple";

struct Fixture {
    database: Database,
    owner_id: Uuid,
    other_id: Uuid,
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

    let owner = BootstrapAdmin::from_parts("owner@example.com", "Owner", PASSWORD).unwrap();
    database
        .provision_bootstrap_admin(&owner, &PasswordService::default())
        .await
        .expect("provisioning must succeed");

    let owner_id = read_uuid(
        &database,
        "SELECT id FROM accounts WHERE is_system_admin = TRUE",
    )
    .await;

    // A second account, created directly: there is no sign-up route, by design (D01).
    let other_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO accounts (id, display_name, email, password_hash) VALUES (?, ?, ?, ?)",
    )
    .bind(other_id.to_string())
    .bind("Autre")
    .bind("other@example.com")
    .bind("$argon2id$v=19$m=65536,t=3,p=4$placeholder")
    .execute(database.pool())
    .await
    .expect("the second account must be insertable");

    Some(Fixture {
        database,
        owner_id,
        other_id,
        _guard: guard,
    })
}

async fn read_uuid(database: &Database, query: &str) -> Uuid {
    let raw: Vec<u8> = sqlx::query_scalar(query)
        .fetch_one(database.pool())
        .await
        .expect("the query must return one column");
    Uuid::parse_str(&String::from_utf8(raw).expect("the column must be UTF-8"))
        .expect("the column must hold a UUID")
}

fn principal(account_id: Uuid, permissions: &[Permission]) -> Principal {
    Principal {
        subject: account_id,
        display_name: "Test".to_owned(),
        roles: BTreeSet::new(),
        permissions: permissions.iter().copied().collect(),
    }
}

#[tokio::test]
async fn creating_a_space_establishes_the_owner_membership_counter_and_grant() {
    let Some(fixture) = fixture().await else {
        return;
    };

    let space = fixture
        .database
        .create_space("Ma collection", fixture.owner_id)
        .await
        .expect("creation must succeed");

    assert_eq!(space.name, "Ma collection");
    assert_eq!(space.owner_account_id, fixture.owner_id);

    // A counter row must exist, otherwise the very first item would be refused.
    let counter: u64 =
        sqlx::query_scalar("SELECT next_number FROM inventory_counters WHERE space_id = ?")
            .bind(space.id.to_string())
            .fetch_one(fixture.database.pool())
            .await
            .expect("the counter must have been created with the space");
    assert_eq!(counter, 1);

    // A space that could actually take an item right away.
    let item = fixture
        .database
        .create_item(space.id, "Appareil photo", fixture.owner_id)
        .await
        .expect("an item must be creatable in a fresh space");
    assert_eq!(item.inventory_number, 1);

    let membership = fixture
        .database
        .membership(space.id, fixture.owner_id)
        .await
        .unwrap()
        .expect("the owner must be a member");
    assert_eq!(
        membership.permissions,
        BTreeSet::from([
            Permission::CollectionsRead,
            Permission::CollectionsWrite,
            Permission::AcquisitionsRead,
            Permission::AcquisitionsWrite,
            Permission::DocumentsRead,
            Permission::DocumentsWrite,
        ]),
        "new spaces explicitly grant collection, acquisitions and documents, not finance"
    );
}

#[tokio::test]
async fn the_owner_does_not_receive_financial_permission() {
    let Some(fixture) = fixture().await else {
        return;
    };

    let space = fixture
        .database
        .create_space("Espace", fixture.owner_id)
        .await
        .unwrap();
    let membership = fixture
        .database
        .membership(space.id, fixture.owner_id)
        .await
        .unwrap()
        .unwrap();

    // The owner holds collections_write in the space, and finance_read application-wide.
    let owner = principal(
        fixture.owner_id,
        &[Permission::CollectionsWrite, Permission::FinanceRead],
    );
    let loaded = membership.as_space_membership();

    assert!(
        owner
            .require_space_permission(space.id, Some(&loaded), Permission::FinanceRead)
            .is_err(),
        "owning a space must not grant access to its amounts"
    );
}

#[tokio::test]
async fn a_member_is_allowed_only_what_was_granted() {
    let Some(fixture) = fixture().await else {
        return;
    };

    let space = fixture
        .database
        .create_space("Partagé", fixture.owner_id)
        .await
        .unwrap();

    // Requirement F02/D14: read-only is the default shape of a share.
    fixture
        .database
        .add_member(space.id, fixture.other_id, &[Permission::CollectionsRead])
        .await
        .expect("adding the member must succeed");

    let space_id = space.id;
    let principal = principal(
        fixture.other_id,
        &[Permission::CollectionsRead, Permission::CollectionsWrite],
    );
    let stored = fixture
        .database
        .membership(space_id, fixture.other_id)
        .await
        .unwrap()
        .unwrap();
    let loaded = stored.as_space_membership();

    // Granted.
    assert!(
        principal
            .require_space_permission(space_id, Some(&loaded), Permission::CollectionsRead)
            .is_ok()
    );

    // Held application-wide, but not granted in this space.
    assert!(
        principal
            .require_space_permission(space_id, Some(&loaded), Permission::CollectionsWrite)
            .is_err(),
        "a write not granted in the space must be refused"
    );

    // Not held application-wide either.
    assert!(
        principal
            .require_space_permission(space_id, Some(&loaded), Permission::FinanceRead)
            .is_err()
    );
}

#[tokio::test]
async fn a_known_space_identifier_does_not_grant_access_to_a_non_member() {
    let Some(fixture) = fixture().await else {
        return;
    };

    let space = fixture
        .database
        .create_space("Privé", fixture.owner_id)
        .await
        .unwrap();

    // The outsider knows the identifier, and even holds every space-scoped permission.
    let outsider = principal(
        fixture.other_id,
        &[
            Permission::CollectionsRead,
            Permission::CollectionsWrite,
            Permission::FinanceRead,
        ],
    );

    let loaded = fixture
        .database
        .membership(space.id, fixture.other_id)
        .await
        .expect("the lookup must succeed");
    assert!(
        loaded.is_none(),
        "the outsider must not be a member of a space it never joined"
    );

    assert!(
        outsider
            .require_space_permission(space.id, None, Permission::CollectionsRead)
            .is_err(),
        "knowing the identifier must never be enough"
    );
}

#[tokio::test]
async fn revoking_a_grant_takes_effect_on_the_next_server_check() {
    let Some(fixture) = fixture().await else {
        return;
    };

    let space = fixture
        .database
        .create_space("Partagé", fixture.owner_id)
        .await
        .unwrap();
    fixture
        .database
        .add_member(
            space.id,
            fixture.other_id,
            &[Permission::CollectionsRead, Permission::CollectionsWrite],
        )
        .await
        .unwrap();

    let principal = principal(
        fixture.other_id,
        &[Permission::CollectionsRead, Permission::CollectionsWrite],
    );

    let before = fixture
        .database
        .membership(space.id, fixture.other_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(before.permissions.len(), 2);

    // The owner downgrades the member to read-only.
    fixture
        .database
        .set_member_permissions(
            space.id,
            fixture.other_id,
            &[Permission::CollectionsRead],
            fixture.owner_id,
        )
        .await
        .expect("the owner may change another member's grants");

    // The server re-reads the grants, so the withdrawn write is gone immediately.
    let after = fixture
        .database
        .membership(space.id, fixture.other_id)
        .await
        .unwrap()
        .unwrap();
    let loaded = after.as_space_membership();

    assert!(
        principal
            .require_space_permission(space.id, Some(&loaded), Permission::CollectionsRead)
            .is_ok()
    );
    assert!(
        principal
            .require_space_permission(space.id, Some(&loaded), Permission::CollectionsWrite)
            .is_err(),
        "a withdrawn grant must stop working, even though the client still holds it"
    );
}

#[tokio::test]
async fn removing_a_member_drops_its_grants_and_its_access() {
    let Some(fixture) = fixture().await else {
        return;
    };

    let space = fixture
        .database
        .create_space("Partagé", fixture.owner_id)
        .await
        .unwrap();
    fixture
        .database
        .add_member(space.id, fixture.other_id, &[Permission::CollectionsRead])
        .await
        .unwrap();

    let removed = fixture
        .database
        .remove_member(space.id, fixture.other_id, fixture.owner_id)
        .await
        .expect("the owner may remove another member");
    assert!(removed);

    assert!(
        fixture
            .database
            .membership(space.id, fixture.other_id)
            .await
            .unwrap()
            .is_none()
    );

    // No orphan grants may remain behind the membership.
    let orphans: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM space_permission_grants WHERE space_id = ? AND account_id = ?",
    )
    .bind(space.id.to_string())
    .bind(fixture.other_id.to_string())
    .fetch_one(fixture.database.pool())
    .await
    .unwrap();
    assert_eq!(orphans, 0, "removing a member must not leave grants behind");
}

#[tokio::test]
async fn a_member_cannot_widen_its_own_grants() {
    let Some(fixture) = fixture().await else {
        return;
    };

    let space = fixture
        .database
        .create_space("Partagé", fixture.owner_id)
        .await
        .unwrap();
    fixture
        .database
        .add_member(space.id, fixture.other_id, &[Permission::CollectionsRead])
        .await
        .unwrap();

    // Even the owner cannot edit its own grants through this route: that would be a way to
    // add finance access to itself without any other control.
    for actor in [fixture.other_id, fixture.owner_id] {
        assert_eq!(
            fixture
                .database
                .set_member_permissions(space.id, actor, &[Permission::FinanceWrite], actor)
                .await
                .err()
                .and_then(|error| error.space_error()),
            Some(SpaceError::CannotChangeOwnGrants)
        );
    }
}

#[tokio::test]
async fn the_owner_cannot_be_removed_from_its_own_space() {
    let Some(fixture) = fixture().await else {
        return;
    };

    let space = fixture
        .database
        .create_space("Privé", fixture.owner_id)
        .await
        .unwrap();

    // A second administrator is authorized to manage this space, but not to remove its owner.
    sqlx::query("UPDATE accounts SET is_system_admin = TRUE WHERE id = ?")
        .bind(fixture.other_id.to_string())
        .execute(fixture.database.pool())
        .await
        .unwrap();

    assert_eq!(
        fixture
            .database
            .remove_member(space.id, fixture.owner_id, fixture.other_id)
            .await
            .err()
            .and_then(|error| error.space_error()),
        Some(SpaceError::OwnerCannotBeRemoved)
    );
}

#[tokio::test]
async fn global_permissions_are_never_stored_in_a_membership() {
    let Some(fixture) = fixture().await else {
        return;
    };

    let space = fixture
        .database
        .create_space("Espace", fixture.owner_id)
        .await
        .unwrap();

    for permission in [
        Permission::AdministrationManage,
        Permission::UsersManage,
        Permission::AuditRead,
        Permission::ReferentialWrite,
        Permission::SynchronizationUse,
    ] {
        assert_eq!(
            fixture
                .database
                .add_member(space.id, fixture.other_id, &[permission])
                .await
                .err()
                .and_then(|error| error.space_error()),
            Some(SpaceError::PermissionNotSpaceScoped),
            "{permission:?} must not be storable as a space grant"
        );
    }

    // Nothing was written by the refused attempts.
    assert!(
        fixture
            .database
            .membership(space.id, fixture.other_id)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn a_space_name_is_trimmed_and_bounded() {
    let Some(fixture) = fixture().await else {
        return;
    };

    let space = fixture
        .database
        .create_space("  Ma collection  ", fixture.owner_id)
        .await
        .unwrap();
    assert_eq!(space.name, "Ma collection");

    for invalid in ["", "   ", &"a".repeat(256)] {
        assert_eq!(
            fixture
                .database
                .create_space(invalid, fixture.owner_id)
                .await
                .err()
                .and_then(|error| error.space_error()),
            Some(SpaceError::InvalidName)
        );
    }
}

#[tokio::test]
async fn duplicates_and_unknown_accounts_are_refused() {
    let Some(fixture) = fixture().await else {
        return;
    };

    let space = fixture
        .database
        .create_space("Espace", fixture.owner_id)
        .await
        .unwrap();
    let missing = Uuid::now_v7();

    assert_eq!(
        fixture
            .database
            .create_space("Autre", missing)
            .await
            .err()
            .and_then(|error| error.space_error()),
        Some(SpaceError::AccountNotFound)
    );

    // The owner is already a member of its own space.
    assert_eq!(
        fixture
            .database
            .add_member(space.id, fixture.owner_id, &[Permission::CollectionsRead])
            .await
            .err()
            .and_then(|error| error.space_error()),
        Some(SpaceError::AlreadyMember)
    );

    assert_eq!(
        fixture
            .database
            .add_member(space.id, missing, &[Permission::CollectionsRead])
            .await
            .err()
            .and_then(|error| error.space_error()),
        Some(SpaceError::AccountNotFound)
    );
}

#[tokio::test]
async fn the_member_list_shows_each_member_with_its_own_grants() {
    let Some(fixture) = fixture().await else {
        return;
    };

    let space = fixture
        .database
        .create_space("Partagé", fixture.owner_id)
        .await
        .unwrap();
    fixture
        .database
        .add_member(
            space.id,
            fixture.other_id,
            &[Permission::CollectionsRead, Permission::DocumentsRead],
        )
        .await
        .unwrap();

    let members = fixture.database.space_members(space.id).await.unwrap();
    assert_eq!(members.len(), 2);

    let other = members
        .iter()
        .find(|member| member.account_id == fixture.other_id)
        .expect("the invited member must be listed");
    assert_eq!(
        other.permissions,
        BTreeSet::from([Permission::CollectionsRead, Permission::DocumentsRead])
    );
}

#[tokio::test]
async fn spaces_start_empty_and_isolated_from_each_other() {
    let Some(fixture) = fixture().await else {
        return;
    };

    let first = fixture
        .database
        .create_space("Premier", fixture.owner_id)
        .await
        .unwrap();
    let second = fixture
        .database
        .create_space("Second", fixture.owner_id)
        .await
        .unwrap();

    fixture
        .database
        .create_item(first.id, "A1", fixture.owner_id)
        .await
        .unwrap();
    fixture
        .database
        .create_item(first.id, "A2", fixture.owner_id)
        .await
        .unwrap();

    // Each space owns its own sequence: the second space still starts at 1.
    let item = fixture
        .database
        .create_item(second.id, "B1", fixture.owner_id)
        .await
        .unwrap();
    assert_eq!(item.inventory_number, 1);
}
