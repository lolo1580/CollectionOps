//! Hierarchical physical locations, single current position and movement history.

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
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
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

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn locations_are_hierarchical_and_movements_are_audited() {
    let Ok(url) = std::env::var("COLLECTIONOPS_TEST_DATABASE_URL") else {
        return;
    };
    let db = Database::connect_and_migrate(&url).await.unwrap();
    collectionops_backend::testing::clear_all(db.pool())
        .await
        .unwrap();
    let admin = BootstrapAdmin::from_parts("owner@example.com", "Owner", PASSWORD).unwrap();
    db.provision_bootstrap_admin(&admin, &PasswordService::default())
        .await
        .unwrap();
    let owner_raw: Vec<u8> = sqlx::query_scalar("SELECT id FROM accounts WHERE email = ?")
        .bind("owner@example.com")
        .fetch_one(db.pool())
        .await
        .unwrap();
    let owner_id = Uuid::parse_str(std::str::from_utf8(&owner_raw).unwrap()).unwrap();
    let reader_id = Uuid::now_v7();
    sqlx::query("INSERT INTO accounts (id, display_name, email, password_hash) SELECT ?, 'Reader', 'reader@example.com', password_hash FROM accounts WHERE email = ?")
        .bind(reader_id.to_string()).bind("owner@example.com").execute(db.pool()).await.unwrap();
    let space = db.create_space("Collection", owner_id).await.unwrap();
    let other = db.create_space("Autre", owner_id).await.unwrap();
    db.add_member(space.id, reader_id, &[Permission::CollectionsRead])
        .await
        .unwrap();
    let router = collectionops_backend::app_with_database(db.clone());
    let owner_token = login(&router, "owner@example.com").await;
    let reader_token = login(&router, "reader@example.com").await;
    let locations_path = format!("/api/v1/spaces/{}/locations", space.id);

    let root = send(
        &router,
        Method::POST,
        &locations_path,
        Some(&owner_token),
        Some(json!({"name":"Pièce", "parent_id":null})),
    )
    .await;
    assert_eq!(root.status(), StatusCode::CREATED);
    let root_id = Uuid::parse_str(body_json(root).await["id"].as_str().unwrap()).unwrap();
    assert_eq!(
        send(
            &router,
            Method::POST,
            &locations_path,
            Some(&owner_token),
            Some(json!({"name":"pièce", "parent_id":null}))
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    let child = send(
        &router,
        Method::POST,
        &locations_path,
        Some(&owner_token),
        Some(json!({"name":"Étagère", "parent_id":root_id})),
    )
    .await;
    assert_eq!(child.status(), StatusCode::CREATED);
    let child_id = Uuid::parse_str(body_json(child).await["id"].as_str().unwrap()).unwrap();
    assert_eq!(
        send(
            &router,
            Method::POST,
            &format!("/api/v1/spaces/{}/locations", other.id),
            Some(&owner_token),
            Some(json!({"name":"Interdit", "parent_id":root_id}))
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(
            &router,
            Method::POST,
            &locations_path,
            Some(&reader_token),
            Some(json!({"name":"Interdit", "parent_id":null}))
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let listed = body_json(
        send(
            &router,
            Method::GET,
            &locations_path,
            Some(&reader_token),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(listed["locations"].as_array().unwrap().len(), 2);

    let item = db.create_item(space.id, "Casque", owner_id).await.unwrap();
    let item_path = format!("/api/v1/items/{}/location", item.id);
    let initial =
        body_json(send(&router, Method::GET, &item_path, Some(&owner_token), None).await).await;
    assert!(initial["location"].is_null());
    assert_eq!(initial["revision"], "1");
    assert_eq!(
        send(
            &router,
            Method::PUT,
            &item_path,
            Some(&reader_token),
            Some(json!({"location_id":root_id, "expected_revision":"1"}))
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let foreign = db.create_location(other.id, None, "Autre").await.unwrap();
    assert_eq!(
        send(
            &router,
            Method::PUT,
            &item_path,
            Some(&owner_token),
            Some(json!({"location_id":foreign.id, "expected_revision":"1"}))
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    let moved = send(
        &router,
        Method::PUT,
        &item_path,
        Some(&owner_token),
        Some(json!({"location_id":root_id, "expected_revision":"1"})),
    )
    .await;
    assert_eq!(moved.status(), StatusCode::OK);
    assert_eq!(body_json(moved).await["revision"], "2");
    assert_eq!(
        send(
            &router,
            Method::PUT,
            &item_path,
            Some(&owner_token),
            Some(json!({"location_id":child_id, "expected_revision":"1"}))
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    let moved = body_json(
        send(
            &router,
            Method::PUT,
            &item_path,
            Some(&owner_token),
            Some(json!({"location_id":child_id, "expected_revision":"2"})),
        )
        .await,
    )
    .await;
    assert_eq!(moved["revision"], "3");
    assert_eq!(moved["location"]["id"], child_id.to_string());
    let no_op = body_json(
        send(
            &router,
            Method::PUT,
            &item_path,
            Some(&owner_token),
            Some(json!({"location_id":child_id, "expected_revision":"3"})),
        )
        .await,
    )
    .await;
    assert_eq!(no_op["revision"], "3");
    let events_path = format!("/api/v1/items/{}/location-events", item.id);
    let events = body_json(
        send(
            &router,
            Method::GET,
            &events_path,
            Some(&reader_token),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(events["events"].as_array().unwrap().len(), 2);
    assert_eq!(events["events"][0]["from_location_name"], "Pièce");
    assert_eq!(events["events"][0]["to_location_name"], "Étagère");

    let transfer = send(
        &router,
        Method::POST,
        &format!("/api/v1/items/{}/transfers", item.id),
        Some(&owner_token),
        Some(json!({"destination_space_id":other.id, "expected_revision":"3"})),
    )
    .await;
    assert_eq!(transfer.status(), StatusCode::OK);
    let current =
        body_json(send(&router, Method::GET, &item_path, Some(&owner_token), None).await).await;
    assert!(current["location"].is_null());
    assert_eq!(current["revision"], "4");
    let destination_events =
        body_json(send(&router, Method::GET, &events_path, Some(&owner_token), None).await).await;
    assert!(destination_events["events"].as_array().unwrap().is_empty());
    let source_events: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM item_location_events WHERE item_id = ? AND space_id = ?",
    )
    .bind(item.id.to_string())
    .bind(space.id.to_string())
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(
        source_events, 3,
        "transfer records a departure in the source space"
    );
}
