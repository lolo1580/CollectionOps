//! A retried item creation must not consume another inventory number.

use axum::{
    body::Body,
    http::{Request, StatusCode},
    response::Response,
};
use collectionops_backend::{BootstrapAdmin, Database, PasswordService, SESSION_TOKEN_HEADER};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

async fn body_json(response: Response) -> Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn same_key_replays_original_item_and_different_name_conflicts() {
    let Ok(url) = std::env::var("COLLECTIONOPS_TEST_DATABASE_URL") else {
        return;
    };
    let database = Database::connect_and_migrate(&url).await.unwrap();
    collectionops_backend::testing::clear_all(database.pool())
        .await
        .unwrap();
    let admin =
        BootstrapAdmin::from_parts("owner@example.com", "Owner", "correct horse battery staple")
            .unwrap();
    database
        .provision_bootstrap_admin(&admin, &PasswordService::default())
        .await
        .unwrap();
    let router = collectionops_backend::app_with_database(database.clone());
    let login = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/sessions")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"email":"owner@example.com","password":"correct horse battery staple"})
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let login = body_json(login).await;
    let token = login["token"].as_str().unwrap();
    let account_id = Uuid::parse_str(login["principal"]["subject"].as_str().unwrap()).unwrap();
    let space = database
        .create_space("Maquettes", account_id)
        .await
        .unwrap();
    let path = format!("/api/v1/spaces/{}/items", space.id);
    let key = Uuid::now_v7();
    let create = |name: &str, key: &str| {
        router.clone().oneshot(
            Request::builder()
                .method("POST")
                .uri(&path)
                .header("content-type", "application/json")
                .header(SESSION_TOKEN_HEADER, token)
                .header("idempotency-key", key)
                .body(Body::from(json!({"name":name}).to_string()))
                .unwrap(),
        )
    };

    let first = create("Mirage 2000B", &key.to_string()).await.unwrap();
    assert_eq!(first.status(), StatusCode::CREATED);
    let first_location = first.headers().get("location").unwrap().clone();
    let first = body_json(first).await;
    assert_eq!(first["inventory_number"], "1");

    let replay = create(" Mirage 2000B ", &key.to_string()).await.unwrap();
    assert_eq!(replay.status(), StatusCode::CREATED);
    assert_eq!(replay.headers().get("location"), Some(&first_location));
    assert_eq!(body_json(replay).await, first);

    let conflict = create("Rafale", &key.to_string()).await.unwrap();
    assert_eq!(conflict.status(), StatusCode::CONFLICT);
    assert_eq!(body_json(conflict).await["code"], "idempotency_key_reused");

    let invalid = create("Autre", "not-a-uuid").await.unwrap();
    assert_eq!(invalid.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body_json(invalid).await["code"], "invalid_idempotency_key");

    let next = create("Autre", &Uuid::now_v7().to_string()).await.unwrap();
    assert_eq!(next.status(), StatusCode::CREATED);
    assert_eq!(body_json(next).await["inventory_number"], "2");

    // A later rename must not change the response saved for the original request.
    let first_id = Uuid::parse_str(first["id"].as_str().unwrap()).unwrap();
    database.rename_item(first_id, "Renomme", 1).await.unwrap();
    let replay_after_edit = create("Mirage 2000B", &key.to_string()).await.unwrap();
    assert_eq!(body_json(replay_after_edit).await, first);

    let destination = database.create_space("Archives", account_id).await.unwrap();
    database
        .transfer_item(first_id, destination.id, 2, account_id)
        .await
        .unwrap();
    let replay_after_transfer = create("Mirage 2000B", &key.to_string()).await.unwrap();
    assert_eq!(body_json(replay_after_transfer).await, first);
    let same_key_other_space = database
        .create_item_with_request_key(destination.id, "Mirage 2000B", account_id, key)
        .await
        .unwrap();
    assert_eq!(same_key_other_space.inventory_number, 2);

    // Concurrent retries must serialize on the request key, before the number is reserved.
    let concurrent_key = Uuid::now_v7();
    let (first, second) = tokio::join!(
        database.create_item_with_request_key(space.id, "Mirage", account_id, concurrent_key),
        database.create_item_with_request_key(space.id, "Mirage", account_id, concurrent_key),
    );
    assert_eq!(first.unwrap(), second.unwrap());
    let next = database
        .create_item(space.id, "Rafale", account_id)
        .await
        .unwrap();
    assert_eq!(next.inventory_number, 4);
}
