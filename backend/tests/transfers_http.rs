//! HTTP contract for protected inventory transfers and history.

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

async fn json_body(response: Response) -> Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

async fn login(router: &Router, email: &str) -> (String, Uuid) {
    let response = send(
        router,
        Method::POST,
        "/api/v1/sessions",
        None,
        Some(json!({"email":email,"password":PASSWORD})),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let payload = json_body(response).await;
    (
        payload["token"].as_str().unwrap().to_owned(),
        Uuid::parse_str(payload["principal"]["subject"].as_str().unwrap()).unwrap(),
    )
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn transfer_requires_both_space_grants_and_preserves_history() {
    let Ok(url) = std::env::var("COLLECTIONOPS_TEST_DATABASE_URL") else {
        return;
    };
    let database = Database::connect_and_migrate(&url).await.unwrap();
    collectionops_backend::testing::clear_all(database.pool())
        .await
        .unwrap();
    let admin = BootstrapAdmin::from_parts("owner@example.com", "Owner", PASSWORD).unwrap();
    database
        .provision_bootstrap_admin(&admin, &PasswordService::default())
        .await
        .unwrap();
    let outsider = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO accounts (id, display_name, email, password_hash) \
        SELECT ?, 'Other', 'other@example.com', password_hash FROM accounts WHERE email = ?",
    )
    .bind(outsider.to_string())
    .bind("owner@example.com")
    .execute(database.pool())
    .await
    .unwrap();
    let router = collectionops_backend::app_with_database(database.clone());
    let (owner_token, owner_id) = login(&router, "owner@example.com").await;
    let (other_token, _) = login(&router, "other@example.com").await;
    let source = database.create_space("Source", owner_id).await.unwrap();
    let destination = database
        .create_space("Destination", owner_id)
        .await
        .unwrap();
    let inaccessible = database.create_space("Prive", outsider).await.unwrap();
    let item = database
        .create_item(source.id, "Objet", owner_id)
        .await
        .unwrap();
    let path = format!("/api/v1/items/{}/transfers", item.id);
    let first = json!({"destination_space_id": destination.id, "expected_revision":"1"});

    assert_eq!(
        send(&router, Method::POST, &path, None, Some(first.clone()))
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send(
            &router,
            Method::POST,
            &path,
            Some(&other_token),
            Some(first.clone())
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(
            &router,
            Method::POST,
            &path,
            Some(&owner_token),
            Some(json!({"destination_space_id": inaccessible.id, "expected_revision":"1"}))
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(
            &router,
            Method::POST,
            &path,
            Some(&owner_token),
            Some(json!({"destination_space_id": destination.id, "expected_revision":"zero"}))
        )
        .await
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );

    let moved = send(
        &router,
        Method::POST,
        &path,
        Some(&owner_token),
        Some(first),
    )
    .await;
    assert_eq!(moved.status(), StatusCode::OK);
    let moved = json_body(moved).await;
    assert_eq!(moved["item"]["space_id"], destination.id.to_string());
    assert_eq!(moved["item"]["revision"], "2");
    assert_eq!(moved["transfer"]["source_inventory_number"], "1");
    assert_eq!(moved["transfer"]["destination_inventory_number"], "1");

    let conflict = send(
        &router,
        Method::POST,
        &path,
        Some(&owner_token),
        Some(json!({"destination_space_id": source.id, "expected_revision":"1"})),
    )
    .await;
    assert_eq!(conflict.status(), StatusCode::CONFLICT);
    assert_eq!(json_body(conflict).await["code"], "revision_conflict");
    assert_eq!(
        send(
            &router,
            Method::POST,
            &path,
            Some(&owner_token),
            Some(json!({"destination_space_id": destination.id, "expected_revision":"2"}))
        )
        .await
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );

    let history = send(&router, Method::GET, &path, Some(&owner_token), None).await;
    assert_eq!(history.status(), StatusCode::OK);
    assert_eq!(
        json_body(history).await["transfers"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    database
        .add_member(destination.id, outsider, &[Permission::CollectionsRead])
        .await
        .unwrap();
    let item_path = format!("/api/v1/items/{}", item.id);
    assert_eq!(
        send(&router, Method::GET, &item_path, Some(&other_token), None)
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        send(&router, Method::GET, &path, Some(&other_token), None)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    database
        .add_member(source.id, outsider, &[Permission::CollectionsRead])
        .await
        .unwrap();
    assert_eq!(
        send(&router, Method::GET, &path, Some(&other_token), None)
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        send(
            &router,
            Method::POST,
            &path,
            Some(&other_token),
            Some(json!({"destination_space_id": source.id, "expected_revision":"2"}))
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    database
        .set_member_permissions(
            destination.id,
            outsider,
            &[Permission::CollectionsRead, Permission::CollectionsWrite],
            owner_id,
        )
        .await
        .unwrap();
    assert_eq!(
        send(
            &router,
            Method::POST,
            &path,
            Some(&other_token),
            Some(json!({"destination_space_id": source.id, "expected_revision":"2"}))
        )
        .await
        .status(),
        StatusCode::FORBIDDEN,
        "write permission is also required in the destination space"
    );
}
