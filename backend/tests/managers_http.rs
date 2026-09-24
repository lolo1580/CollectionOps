//! Delegated member administration through the authenticated API.

use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode, header},
    response::Response,
};
use collectionops_backend::{
    BootstrapAdmin, Database, PasswordService, Permission, SESSION_TOKEN_HEADER,
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const PASSWORD: &str = "correct horse battery staple";

async fn send(
    router: &Router,
    method: Method,
    path: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> Response {
    let mut request = Request::builder().method(method).uri(path);
    if let Some(token) = token {
        request = request.header(SESSION_TOKEN_HEADER, token);
    }
    if body.is_some() {
        request = request.header(header::CONTENT_TYPE, "application/json");
    }
    router
        .clone()
        .oneshot(
            request
                .body(body.map_or_else(Body::empty, |value| Body::from(value.to_string())))
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn body_json(response: Response) -> Value {
    serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap()
}

async fn login(router: &Router, email: &str) -> String {
    let response = send(
        router,
        Method::POST,
        "/api/v1/sessions",
        None,
        Some(json!({"email":email,"password":PASSWORD})),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    body_json(response).await["token"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn insert_account(db: &Database, email: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO accounts (id, display_name, email, password_hash) SELECT ?, ?, ?, password_hash FROM accounts WHERE email = 'admin@example.com'")
        .bind(id.to_string()).bind(email).bind(email).execute(db.pool()).await.unwrap();
    id
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn delegated_managers_manage_members_without_escalating() {
    let Ok(url) = std::env::var("COLLECTIONOPS_TEST_DATABASE_URL") else {
        return;
    };
    let db = Database::connect_and_migrate(&url).await.unwrap();
    collectionops_backend::testing::clear_all(db.pool())
        .await
        .unwrap();
    let admin = BootstrapAdmin::from_parts("admin@example.com", "Admin", PASSWORD).unwrap();
    db.provision_bootstrap_admin(&admin, &PasswordService::default())
        .await
        .unwrap();
    // A regular owner: the bootstrap administrator would bypass the delegation checks.
    let owner = insert_account(&db, "owner@example.com").await;
    let manager = insert_account(&db, "manager@example.com").await;
    let member = insert_account(&db, "member@example.com").await;
    let stranger = insert_account(&db, "stranger@example.com").await;
    let space = db.create_space("Collection", owner).await.unwrap();
    db.add_member(
        space.id,
        manager,
        &[Permission::CollectionsRead, Permission::CollectionsWrite],
    )
    .await
    .unwrap();
    db.add_member(space.id, member, &[Permission::CollectionsRead])
        .await
        .unwrap();
    let router = collectionops_backend::app_with_database(db.clone());
    let owner_token = login(&router, "owner@example.com").await;
    let manager_token = login(&router, "manager@example.com").await;
    let members_path = format!("/api/v1/spaces/{}/members", space.id);
    let managers_path = format!("/api/v1/spaces/{}/managers", space.id);
    let manager_path = format!("{managers_path}/{manager}");

    // Before being appointed, the member cannot see the delegation.
    assert_eq!(
        send(
            &router,
            Method::GET,
            &managers_path,
            Some(&manager_token),
            None
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    // The owner appoints an existing member as manager.
    assert_eq!(
        send(
            &router,
            Method::PUT,
            &manager_path,
            Some(&owner_token),
            None
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        send(
            &router,
            Method::PUT,
            &manager_path,
            Some(&owner_token),
            None
        )
        .await
        .status(),
        StatusCode::CONFLICT,
        "a manager is appointed once"
    );
    assert_eq!(
        send(
            &router,
            Method::PUT,
            &format!("{managers_path}/{owner}"),
            Some(&owner_token),
            None
        )
        .await
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY,
        "the owner already manages the space"
    );
    assert_eq!(
        send(
            &router,
            Method::PUT,
            &format!("{managers_path}/{stranger}"),
            Some(&owner_token),
            None
        )
        .await
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY,
        "only a member can be a manager"
    );
    // The manager can now read the delegation and the members.
    let managers = body_json(
        send(
            &router,
            Method::GET,
            &managers_path,
            Some(&manager_token),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(managers["managers"].as_array().unwrap().len(), 1);
    assert_eq!(managers["managers"][0]["account_id"], manager.to_string());
    assert_eq!(
        send(
            &router,
            Method::GET,
            &members_path,
            Some(&manager_token),
            None
        )
        .await
        .status(),
        StatusCode::OK
    );
    // A manager may pass on a right it holds itself, but never one it does not hold.
    assert_eq!(
        send(
            &router,
            Method::PUT,
            &format!("{members_path}/{member}/permissions"),
            Some(&manager_token),
            Some(json!({"permissions":["collections_read"]}))
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        send(
            &router,
            Method::PUT,
            &format!("{members_path}/{member}/permissions"),
            Some(&manager_token),
            Some(json!({"permissions":["finance_read"]}))
        )
        .await
        .status(),
        StatusCode::FORBIDDEN,
        "a manager cannot grant a right it does not hold"
    );
    // A manager may never touch the owner nor remove it.
    assert_eq!(
        send(
            &router,
            Method::PUT,
            &format!("{members_path}/{owner}/permissions"),
            Some(&manager_token),
            Some(json!({"permissions":["collections_read","collections_write"]}))
        )
        .await
        .status(),
        StatusCode::FORBIDDEN,
        "a manager cannot rewrite the owner's grants"
    );
    assert_eq!(
        send(
            &router,
            Method::DELETE,
            &format!("{members_path}/{owner}"),
            Some(&manager_token),
            None
        )
        .await
        .status(),
        StatusCode::FORBIDDEN,
        "the owner cannot be removed"
    );
    // A manager cannot appoint or revoke another manager.
    assert_eq!(
        send(
            &router,
            Method::PUT,
            &format!("{managers_path}/{member}"),
            Some(&manager_token),
            None
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(
            &router,
            Method::DELETE,
            &manager_path,
            Some(&manager_token),
            None
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    // A manager can remove another member.
    assert_eq!(
        send(
            &router,
            Method::DELETE,
            &format!("{members_path}/{member}"),
            Some(&manager_token),
            None
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    let audit: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM space_manager_events WHERE space_id = ? AND event_code = 'manager_appointed'",
    )
    .bind(space.id.to_string())
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(audit, 1, "the appointment is audited");

    // Revocation restores the previous restriction.
    assert_eq!(
        send(
            &router,
            Method::DELETE,
            &manager_path,
            Some(&owner_token),
            None
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        send(
            &router,
            Method::GET,
            &members_path,
            Some(&manager_token),
            None
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    // The manager kept only its explicit grants: delegation never widened them.
    let rights = body_json(
        send(
            &router,
            Method::GET,
            &format!("/api/v1/spaces/{}/my-permissions", space.id),
            Some(&manager_token),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(
        rights["permissions"],
        json!(["collections_read", "collections_write"])
    );

    // Removing a member who is a manager also removes the delegation (cascade).
    assert_eq!(
        send(
            &router,
            Method::PUT,
            &manager_path,
            Some(&owner_token),
            None
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        send(
            &router,
            Method::DELETE,
            &format!("{members_path}/{manager}"),
            Some(&owner_token),
            None
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    let managers = body_json(
        send(
            &router,
            Method::GET,
            &managers_path,
            Some(&owner_token),
            None,
        )
        .await,
    )
    .await;
    assert!(managers["managers"].as_array().unwrap().is_empty());
}
