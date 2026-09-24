//! Directed item-to-item relations through the authenticated API.

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

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn item_relations_stay_inside_the_space_and_survive_state_changes() {
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
    let item = db.create_item(space.id, "Casque", owner_id).await.unwrap();
    let variant = db
        .create_item(space.id, "Casque M1", owner_id)
        .await
        .unwrap();
    let related = db.create_item(space.id, "Gants", owner_id).await.unwrap();
    let foreign = db
        .create_item(other.id, "Ailleurs", owner_id)
        .await
        .unwrap();
    let router = collectionops_backend::app_with_database(db.clone());
    let owner_token = login(&router, "owner@example.com").await;
    let reader_token = login(&router, "reader@example.com").await;
    let path = format!("/api/v1/items/{}/relations", item.id);

    assert_eq!(
        send(&router, Method::GET, &path, None, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send(
            &router,
            Method::PUT,
            &path,
            Some(&reader_token),
            Some(json!({"relations":[{"target_id":variant.id,"kind":"related"}],"expected_revision":"1"}))
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    // A relation must not point at the item itself.
    assert_eq!(
        send(
            &router,
            Method::PUT,
            &path,
            Some(&owner_token),
            Some(json!({"relations":[{"target_id":item.id,"kind":"related"}],"expected_revision":"1"}))
        )
        .await
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    // The same target and kind cannot be declared twice.
    assert_eq!(
        send(
            &router,
            Method::PUT,
            &path,
            Some(&owner_token),
            Some(json!({"relations":[
                {"target_id":variant.id,"kind":"related"},
                {"target_id":variant.id,"kind":"related"}
            ],"expected_revision":"1"}))
        )
        .await
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    // An unknown kind is refused before any write.
    assert_eq!(
        send(
            &router,
            Method::PUT,
            &path,
            Some(&owner_token),
            Some(json!({"relations":[{"target_id":variant.id,"kind":"sibling"}],"expected_revision":"1"}))
        )
        .await
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    // A target in another space is invisible even by identifier.
    assert_eq!(
        send(
            &router,
            Method::PUT,
            &path,
            Some(&owner_token),
            Some(json!({"relations":[{"target_id":foreign.id,"kind":"related"}],"expected_revision":"1"}))
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(
            &router,
            Method::PUT,
            &path,
            Some(&owner_token),
            Some(json!({"relations":[{"target_id":variant.id,"kind":"related"}],"expected_revision":"9"}))
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );

    let replaced = send(
        &router,
        Method::PUT,
        &path,
        Some(&owner_token),
        Some(json!({"relations":[
            {"target_id":variant.id,"kind":"variant_of"},
            {"target_id":related.id,"kind":"related"}
        ],"expected_revision":"1"})),
    )
    .await;
    assert_eq!(replaced.status(), StatusCode::OK);
    let replaced = body_json(replaced).await;
    assert_eq!(replaced["revision"], "2");
    assert_eq!(replaced["relations"].as_array().unwrap().len(), 2);
    let names: Vec<&str> = replaced["relations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|relation| relation["related_item_name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"Casque M1"));
    assert!(names.contains(&"Gants"));
    assert!(
        replaced["relations"]
            .as_array()
            .unwrap()
            .iter()
            .all(|relation| relation["direction"] == "outgoing")
    );

    // The other endpoint sees the same link as incoming.
    let incoming = body_json(
        send(
            &router,
            Method::GET,
            &format!("/api/v1/items/{}/relations", variant.id),
            Some(&reader_token),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(incoming["revision"], "1");
    assert_eq!(incoming["relations"].as_array().unwrap().len(), 1);
    assert_eq!(incoming["relations"][0]["direction"], "incoming");
    assert_eq!(incoming["relations"][0]["kind"], "variant_of");
    assert_eq!(
        incoming["relations"][0]["related_item_id"],
        item.id.to_string()
    );
    assert_eq!(incoming["relations"][0]["related_item_name"], "Casque");

    // An identical selection keeps the revision and does not rewrite rows.
    let no_op = body_json(
        send(
            &router,
            Method::PUT,
            &path,
            Some(&owner_token),
            Some(json!({"relations":[
                {"target_id":variant.id,"kind":"variant_of"},
                {"target_id":related.id,"kind":"related"}
            ],"expected_revision":"2"})),
        )
        .await,
    )
    .await;
    assert_eq!(no_op["revision"], "2");
    assert_eq!(no_op["relations"].as_array().unwrap().len(), 2);

    // Too many entries are refused up front.
    let too_many: Vec<Value> = (0..51)
        .map(|_| json!({"target_id":variant.id,"kind":"related"}))
        .collect();
    assert_eq!(
        send(
            &router,
            Method::PUT,
            &path,
            Some(&owner_token),
            Some(json!({"relations":too_many,"expected_revision":"2"}))
        )
        .await
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );

    // A trashed item keeps its links readable but refuses new ones.
    assert_eq!(
        send(
            &router,
            Method::POST,
            &format!("/api/v1/items/{}/trash", item.id),
            Some(&owner_token),
            Some(json!({"expected_revision":"2"})),
        )
        .await
        .status(),
        StatusCode::OK
    );
    let after_trash =
        body_json(send(&router, Method::GET, &path, Some(&reader_token), None).await).await;
    assert_eq!(after_trash["relations"].as_array().unwrap().len(), 2);
    assert_eq!(
        send(
            &router,
            Method::PUT,
            &path,
            Some(&owner_token),
            Some(json!({"relations":[],"expected_revision":"3"}))
        )
        .await
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        send(
            &router,
            Method::POST,
            &format!("/api/v1/items/{}/restore", item.id),
            Some(&owner_token),
            Some(json!({"expected_revision":"3"})),
        )
        .await
        .status(),
        StatusCode::OK
    );

    // A transfer removes every link that mentions the moved item.
    let transfer = send(
        &router,
        Method::POST,
        &format!("/api/v1/items/{}/transfers", item.id),
        Some(&owner_token),
        Some(json!({"destination_space_id":other.id,"expected_revision":"4"})),
    )
    .await;
    assert_eq!(transfer.status(), StatusCode::OK);
    let moved = db.item_relations(item.id).await.unwrap();
    assert!(moved.is_empty());
    let orphan = db.item_relations(variant.id).await.unwrap();
    assert!(orphan.is_empty());
}
