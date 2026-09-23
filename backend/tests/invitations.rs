//! Invitation lifecycle against the isolated MariaDB test database.
use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use collectionops_backend::{
    BootstrapAdmin, Database, EmailAddress, InvitationError, InvitationToken, PasswordService,
    Permission,
};
use serde_json::json;
use tokio::sync::{Mutex, MutexGuard};
use tower::ServiceExt;
use uuid::Uuid;

static DATABASE_LOCK: Mutex<()> = Mutex::const_new(());
const PASSWORD: &str = "correct horse battery staple";

struct Fixture {
    db: Database,
    owner: Uuid,
    space: Uuid,
    _guard: MutexGuard<'static, ()>,
}

async fn fixture() -> Option<Fixture> {
    let url = std::env::var("COLLECTIONOPS_TEST_DATABASE_URL").ok()?;
    assert!(
        url.ends_with("/collectionops_test"),
        "refusing to clear a non-test database"
    );
    let guard = DATABASE_LOCK.lock().await;
    let db = Database::connect_and_migrate(&url)
        .await
        .expect("test MariaDB must be reachable");
    collectionops_backend::testing::clear_all(db.pool())
        .await
        .expect("test database must clear");
    let admin = BootstrapAdmin::from_parts("owner@example.com", "Owner", PASSWORD).unwrap();
    db.provision_bootstrap_admin(&admin, &PasswordService::default())
        .await
        .unwrap();
    let raw: Vec<u8> =
        sqlx::query_scalar("SELECT id FROM accounts WHERE email = 'owner@example.com'")
            .fetch_one(db.pool())
            .await
            .unwrap();
    let owner = Uuid::parse_str(std::str::from_utf8(&raw).unwrap()).unwrap();
    let space = db.create_space("Collection", owner).await.unwrap().id;
    Some(Fixture {
        db,
        owner,
        space,
        _guard: guard,
    })
}

fn token_from_link(link: &str) -> InvitationToken {
    InvitationToken::parse(link.rsplit('/').next().unwrap()).unwrap()
}

#[tokio::test]
async fn new_account_accepts_once_with_read_only_grant_and_audit() {
    let Some(f) = fixture().await else { return };
    let email = EmailAddress::parse("invitee@example.com").unwrap();
    let issued =
        f.db.issue_invitation(f.space, f.owner, &email)
            .await
            .unwrap();
    let link = issued.link();
    assert!(link.starts_with("collectionops://invite/"));
    assert!(!format!("{:?}", issued.invitation).contains(&link));
    let fingerprint: Vec<u8> =
        sqlx::query_scalar("SELECT token_fingerprint FROM space_invitations WHERE id = ?")
            .bind(issued.invitation.id.to_string())
            .fetch_one(f.db.pool())
            .await
            .unwrap();
    assert_eq!(fingerprint.len(), 32);
    assert!(
        !fingerprint
            .windows(8)
            .any(|part| link.as_bytes().windows(8).any(|secret| secret == part))
    );

    let account =
        f.db.accept_invitation(
            issued.invitation.id,
            &token_from_link(&link),
            None,
            Some(("Invité", PASSWORD)),
            &PasswordService::default(),
        )
        .await
        .unwrap();
    let member = f.db.membership(f.space, account).await.unwrap().unwrap();
    assert_eq!(member.permissions.len(), 1);
    assert!(member.permissions.contains(&Permission::CollectionsRead));
    let verified: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM accounts WHERE id = ? AND email_verified_at IS NOT NULL",
    )
    .bind(account.to_string())
    .fetch_one(f.db.pool())
    .await
    .unwrap();
    assert_eq!(verified, 1);
    let audit_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM space_audit_events WHERE invitation_id = ?")
            .bind(issued.invitation.id.to_string())
            .fetch_one(f.db.pool())
            .await
            .unwrap();
    assert_eq!(audit_count, 2);
    assert!(matches!(
        f.db.accept_invitation(
            issued.invitation.id,
            &token_from_link(&link),
            None,
            Some(("Autre", PASSWORD)),
            &PasswordService::default()
        )
        .await,
        Err(InvitationError::NotFound)
    ));
}

#[tokio::test]
async fn existing_account_requires_matching_verified_session() {
    let Some(f) = fixture().await else { return };
    let email = EmailAddress::parse("existing@example.com").unwrap();
    let id = Uuid::now_v7();
    let hash = PasswordService::default().hash_password(PASSWORD).unwrap();
    sqlx::query("INSERT INTO accounts (id, display_name, email, password_hash, email_verified_at) VALUES (?, 'Existing', ?, ?, CURRENT_TIMESTAMP(6))")
        .bind(id.to_string()).bind(email.as_str()).bind(hash.as_str()).execute(f.db.pool()).await.unwrap();
    let issued =
        f.db.issue_invitation(f.space, f.owner, &email)
            .await
            .unwrap();
    let token = token_from_link(&issued.link());
    assert!(matches!(
        f.db.accept_invitation(
            issued.invitation.id,
            &token,
            None,
            Some(("Takeover", PASSWORD)),
            &PasswordService::default()
        )
        .await,
        Err(InvitationError::ExistingAccountRequiresLogin)
    ));
    assert!(matches!(
        f.db.accept_invitation(
            issued.invitation.id,
            &token,
            Some(f.owner),
            None,
            &PasswordService::default()
        )
        .await,
        Err(InvitationError::ExistingAccountRequiresLogin)
    ));
    f.db.accept_invitation(
        issued.invitation.id,
        &token,
        Some(id),
        None,
        &PasswordService::default(),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn reissue_invalidates_previous_link_and_revocation_stops_acceptance() {
    let Some(f) = fixture().await else { return };
    let email = EmailAddress::parse("other@example.com").unwrap();
    let first =
        f.db.issue_invitation(f.space, f.owner, &email)
            .await
            .unwrap();
    let second =
        f.db.issue_invitation(f.space, f.owner, &email)
            .await
            .unwrap();
    assert!(matches!(
        f.db.accept_invitation(
            first.invitation.id,
            &token_from_link(&first.link()),
            None,
            Some(("First", PASSWORD)),
            &PasswordService::default()
        )
        .await,
        Err(InvitationError::NotFound)
    ));
    let listed = f.db.list_invitations(f.space, f.owner).await.unwrap();
    assert_eq!(listed.len(), 2);
    assert!(
        listed
            .iter()
            .any(|i| i.id == first.invitation.id && i.revoked_at.is_some())
    );
    f.db.revoke_invitation(f.space, second.invitation.id, f.owner)
        .await
        .unwrap();
    assert!(matches!(
        f.db.accept_invitation(
            second.invitation.id,
            &token_from_link(&second.link()),
            None,
            Some(("Second", PASSWORD)),
            &PasswordService::default()
        )
        .await,
        Err(InvitationError::NotFound)
    ));
}

#[tokio::test]
async fn http_accepts_new_account_but_never_echoes_the_secret() {
    let Some(f) = fixture().await else { return };
    let issued =
        f.db.issue_invitation(
            f.space,
            f.owner,
            &EmailAddress::parse("http@example.com").unwrap(),
        )
        .await
        .unwrap();
    let link = issued.link();
    let router = collectionops_backend::app_with_database(f.db.clone());
    let request = Request::builder()
        .method("POST")
        .uri(format!(
            "/api/v1/invitations/{}/accept",
            issued.invitation.id
        ))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({"token": token_from_link(&link).expose_secret(),
            "display_name": "HTTP User", "password": PASSWORD})
            .to_string(),
        ))
        .unwrap();
    let response = router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let replay = Request::builder()
        .method("POST")
        .uri(format!(
            "/api/v1/invitations/{}/accept",
            issued.invitation.id
        ))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({"token": token_from_link(&link).expose_secret(),
            "display_name": "HTTP User", "password": PASSWORD})
            .to_string(),
        ))
        .unwrap();
    let response = router.oneshot(replay).await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn non_owner_cannot_issue_list_or_revoke() {
    let Some(f) = fixture().await else { return };
    let other = Uuid::now_v7();
    let email = EmailAddress::parse("other@example.com").unwrap();
    assert!(matches!(
        f.db.issue_invitation(f.space, other, &email).await,
        Err(InvitationError::NotOwner)
    ));
    let issued =
        f.db.issue_invitation(f.space, f.owner, &email)
            .await
            .unwrap();
    assert!(matches!(
        f.db.list_invitations(f.space, other).await,
        Err(InvitationError::NotOwner)
    ));
    assert!(matches!(
        f.db.revoke_invitation(f.space, issued.invitation.id, other)
            .await,
        Err(InvitationError::NotOwner)
    ));
    let wrong = InvitationToken::generate().unwrap();
    assert!(matches!(
        f.db.accept_invitation(
            issued.invitation.id,
            &wrong,
            None,
            Some(("Other", PASSWORD)),
            &PasswordService::default()
        )
        .await,
        Err(InvitationError::NotFound)
    ));
}
