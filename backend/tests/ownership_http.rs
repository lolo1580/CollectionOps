//! Space ownership transfer through the authenticated API.

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
async fn ownership_transfer_requires_the_owner_and_an_existing_member() {
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
    // The owner is a regular account: the bootstrap administrator keeps its system-wide rights,
    // which would hide a missing ownership check.
    let owner_id = insert_account(&db, "owner@example.com").await;
    let member = insert_account(&db, "member@example.com").await;
    let stranger = insert_account(&db, "stranger@example.com").await;
    let space = db.create_space("Collection", owner_id).await.unwrap();
    db.add_member(space.id, member, &[Permission::CollectionsRead])
        .await
        .unwrap();
    let router = collectionops_backend::app_with_database(db.clone());
    let owner_token = login(&router, "owner@example.com").await;
    let member_token = login(&router, "member@example.com").await;
    let path = format!("/api/v1/spaces/{}/ownership", space.id);

    assert_eq!(
        send(
            &router,
            Method::POST,
            &path,
            None,
            Some(json!({"new_owner_account_id": member}))
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    // A plain member cannot transfer ownership, and must not learn the space is transferable.
    assert_eq!(
        send(
            &router,
            Method::POST,
            &path,
            Some(&member_token),
            Some(json!({"new_owner_account_id": member}))
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    // The target must already belong to the space.
    assert_eq!(
        send(
            &router,
            Method::POST,
            &path,
            Some(&owner_token),
            Some(json!({"new_owner_account_id": stranger}))
        )
        .await
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    // Transferring to the current owner is a no-op that is refused.
    assert_eq!(
        send(
            &router,
            Method::POST,
            &path,
            Some(&owner_token),
            Some(json!({"new_owner_account_id": owner_id}))
        )
        .await
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    // An unknown space is not found.
    assert_eq!(
        send(
            &router,
            Method::POST,
            &format!("/api/v1/spaces/{}/ownership", Uuid::now_v7()),
            Some(&owner_token),
            Some(json!({"new_owner_account_id": member}))
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );

    let transferred = send(
        &router,
        Method::POST,
        &path,
        Some(&owner_token),
        Some(json!({"new_owner_account_id": member})),
    )
    .await;
    assert_eq!(transferred.status(), StatusCode::OK);
    let transferred = body_json(transferred).await;
    assert_eq!(transferred["space_id"], space.id.to_string());
    assert_eq!(
        transferred["previous_owner_account_id"],
        owner_id.to_string()
    );
    assert_eq!(transferred["new_owner_account_id"], member.to_string());
    assert_eq!(transferred["actor_account_id"], owner_id.to_string());

    let stored: Vec<u8> = sqlx::query_scalar("SELECT owner_account_id FROM spaces WHERE id = ?")
        .bind(space.id.to_string())
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(
        Uuid::parse_str(std::str::from_utf8(&stored).unwrap()).unwrap(),
        member
    );
    let events: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM space_ownership_events WHERE space_id = ? AND previous_owner_account_id = ? AND new_owner_account_id = ?",
    )
    .bind(space.id.to_string())
    .bind(owner_id.to_string())
    .bind(member.to_string())
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(events, 1, "the transfer must be audited once");

    // The mandate really moved: the new owner manages the members, the old owner no longer does.
    assert_eq!(
        send(
            &router,
            Method::GET,
            &format!("/api/v1/spaces/{}/members", space.id),
            Some(&member_token),
            None
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        send(
            &router,
            Method::GET,
            &format!("/api/v1/spaces/{}/members", space.id),
            Some(&owner_token),
            None
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    // The previous owner stays a member with the same grants.
    let members = body_json(
        send(
            &router,
            Method::GET,
            &format!("/api/v1/spaces/{}/members", space.id),
            Some(&member_token),
            None,
        )
        .await,
    )
    .await;
    let previous = members["members"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["account_id"] == owner_id.to_string())
        .expect("the previous owner must remain a member");
    assert_eq!(previous["permissions"].as_array().unwrap().len(), 4);

    // Ownership grants no collection or financial right by itself.
    let rights = body_json(
        send(
            &router,
            Method::GET,
            &format!("/api/v1/spaces/{}/my-permissions", space.id),
            Some(&member_token),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(rights["permissions"], json!(["collections_read"]));

    // The new owner can transfer it back, replacing the owner.
    let back = send(
        &router,
        Method::POST,
        &path,
        Some(&member_token),
        Some(json!({"new_owner_account_id": owner_id})),
    )
    .await;
    assert_eq!(back.status(), StatusCode::OK);
    assert_eq!(
        body_json(back).await["previous_owner_account_id"],
        member.to_string()
    );
}
