//! Item details and space-scoped series/groupings through the authenticated API.

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
async fn details_groups_filters_and_transfer_obey_space_and_revision() {
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
    let router = collectionops_backend::app_with_database(db.clone());
    let owner_token = login(&router, "owner@example.com").await;
    let reader_token = login(&router, "reader@example.com").await;
    let details_path = format!("/api/v1/items/{}/details", item.id);
    let groups_path = format!("/api/v1/spaces/{}/groups", space.id);
    let item_groups_path = format!("/api/v1/items/{}/groups", item.id);

    assert_eq!(
        send(
            &router,
            Method::PUT,
            &details_path,
            Some(&reader_token),
            Some(json!({"description":"Interdit","expected_revision":"1"}))
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let updated = send(
        &router,
        Method::PUT,
        &details_path,
        Some(&owner_token),
        Some(
            json!({"description":"  Pièce d'époque  ","historical_reference":" Archive 42 ",
            "technical_reference":" Modèle B ","expected_revision":"1"}),
        ),
    )
    .await;
    assert_eq!(updated.status(), StatusCode::OK);
    let updated = body_json(updated).await;
    assert_eq!(updated["description"], "Pièce d'époque");
    assert_eq!(updated["historical_reference"], "Archive 42");
    assert_eq!(updated["technical_reference"], "Modèle B");
    assert_eq!(updated["revision"], "2");
    assert_eq!(
        send(
            &router,
            Method::PUT,
            &details_path,
            Some(&owner_token),
            Some(json!({"description":"Ancien","expected_revision":"1"}))
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );

    let created = send(
        &router,
        Method::POST,
        &groups_path,
        Some(&owner_token),
        Some(json!({"kind":"series","name":"Casques 1944"})),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let created = body_json(created).await;
    let group_id = created["id"].as_str().unwrap().to_owned();
    assert_eq!(created["revision"], "1");
    let group_path = format!("{groups_path}/{group_id}");
    assert_eq!(
        send(
            &router,
            Method::PATCH,
            &group_path,
            Some(&reader_token),
            Some(json!({"name":"Interdit","expected_revision":"1"}))
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(
            &router,
            Method::DELETE,
            &group_path,
            Some(&reader_token),
            Some(json!({"expected_revision":"1"}))
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let renamed = send(
        &router,
        Method::PATCH,
        &group_path,
        Some(&owner_token),
        Some(json!({"name":"Casques 1945","expected_revision":"1"})),
    )
    .await;
    assert_eq!(renamed.status(), StatusCode::OK);
    let renamed = body_json(renamed).await;
    assert_eq!(renamed["id"], group_id);
    assert_eq!(renamed["revision"], "2");
    assert_eq!(
        send(
            &router,
            Method::PATCH,
            &group_path,
            Some(&owner_token),
            Some(json!({"name":"Nom dépassé","expected_revision":"1"}))
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        send(
            &router,
            Method::POST,
            &groups_path,
            Some(&owner_token),
            Some(json!({"kind":"series","name":"Casques 1945"}))
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        send(
            &router,
            Method::POST,
            &groups_path,
            Some(&reader_token),
            Some(json!({"kind":"group","name":"Interdit"}))
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let foreign = db
        .create_group(other.id, "group", "Autre groupe")
        .await
        .unwrap();
    assert_eq!(
        send(
            &router,
            Method::PUT,
            &item_groups_path,
            Some(&owner_token),
            Some(json!({"group_ids":[foreign.id],"expected_revision":"2"}))
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    let assigned = send(
        &router,
        Method::PUT,
        &item_groups_path,
        Some(&owner_token),
        Some(json!({"group_ids":[group_id],"expected_revision":"2"})),
    )
    .await;
    assert_eq!(assigned.status(), StatusCode::OK);
    assert_eq!(body_json(assigned).await["revision"], "3");
    assert_eq!(
        send(
            &router,
            Method::DELETE,
            &group_path,
            Some(&owner_token),
            Some(json!({"expected_revision":"2"}))
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    let filtered = send(
        &router,
        Method::GET,
        &format!("/api/v1/spaces/{}/items?group_id={group_id}", space.id),
        Some(&reader_token),
        None,
    )
    .await;
    assert_eq!(filtered.status(), StatusCode::OK);
    assert_eq!(
        body_json(filtered).await["items"].as_array().unwrap().len(),
        1
    );
    assert_eq!(
        send(
            &router,
            Method::GET,
            &format!("/api/v1/spaces/{}/items?group_id={group_id}", other.id),
            Some(&owner_token),
            None
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );

    let transfer = send(
        &router,
        Method::POST,
        &format!("/api/v1/items/{}/transfers", item.id),
        Some(&owner_token),
        Some(json!({"destination_space_id":other.id,"expected_revision":"3"})),
    )
    .await;
    assert_eq!(transfer.status(), StatusCode::OK);
    let memberships = db.item_groups(item.id).await.unwrap();
    assert!(memberships.is_empty());
    assert_eq!(
        send(
            &router,
            Method::DELETE,
            &group_path,
            Some(&owner_token),
            Some(json!({"expected_revision":"1"}))
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        send(
            &router,
            Method::DELETE,
            &group_path,
            Some(&owner_token),
            Some(json!({"expected_revision":"2"}))
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    assert!(
        db.group(space.id, Uuid::parse_str(&group_id).unwrap())
            .await
            .is_err()
    );
    assert_eq!(
        db.item(item.id).await.unwrap().description.as_deref(),
        Some("Pièce d'époque")
    );
}
