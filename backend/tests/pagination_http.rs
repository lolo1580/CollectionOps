//! Search and keyset pagination of a space inventory.

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

async fn payload(response: Response) -> Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn inventory_pages_are_bounded_and_search_treats_wildcards_literally() {
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
    let login = payload(login).await;
    let token = login["token"].as_str().unwrap();
    let account_id = Uuid::parse_str(login["principal"]["subject"].as_str().unwrap()).unwrap();
    let space = database
        .create_space("Catalogue", account_id)
        .await
        .unwrap();
    for name in ["Alpha", "Beta", "100% Casque", "casque"] {
        database
            .create_item(space.id, name, account_id)
            .await
            .unwrap();
    }
    let path = format!("/api/v1/spaces/{}/items", space.id);
    let get = |uri: String| {
        router.clone().oneshot(
            Request::builder()
                .uri(uri)
                .header(SESSION_TOKEN_HEADER, token)
                .body(Body::empty())
                .unwrap(),
        )
    };

    let first = get(format!("{path}?limit=2")).await.unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let first = payload(first).await;
    assert_eq!(first["items"].as_array().unwrap().len(), 2);
    assert_eq!(first["items"][0]["name"], "Alpha");
    assert_eq!(first["next_cursor"], "2");

    let second = payload(get(format!("{path}?limit=2&after=2")).await.unwrap()).await;
    assert_eq!(second["items"].as_array().unwrap().len(), 2);
    assert_eq!(second["items"][0]["name"], "100% Casque");
    assert!(second["next_cursor"].is_null());

    let searched = payload(get(format!("{path}?q=casque")).await.unwrap()).await;
    assert_eq!(searched["items"].as_array().unwrap().len(), 2);
    let literal = payload(get(format!("{path}?q=%25")).await.unwrap()).await;
    assert_eq!(literal["items"].as_array().unwrap().len(), 1);
    assert_eq!(literal["items"][0]["name"], "100% Casque");

    let listed = database.items_in_space(space.id).await.unwrap();
    database
        .update_item_details(
            listed[0].id,
            Some("Boîte en carton"),
            None,
            Some("REF-42"),
            1,
        )
        .await
        .unwrap();
    let by_description = payload(get(format!("{path}?q=carton")).await.unwrap()).await;
    assert_eq!(by_description["items"].as_array().unwrap().len(), 1);
    assert_eq!(by_description["items"][0]["name"], "Alpha");
    let by_reference = payload(get(format!("{path}?q=REF-42")).await.unwrap()).await;
    assert_eq!(by_reference["items"].as_array().unwrap().len(), 1);
    assert_eq!(by_reference["items"][0]["name"], "Alpha");

    let root = database
        .create_category(space.id, None, "Militaire")
        .await
        .unwrap();
    let child = database
        .create_category(space.id, Some(root.id), "Casques")
        .await
        .unwrap();
    let items = database.items_in_space(space.id).await.unwrap();
    database
        .add_item_categories(items[2].id, 1, &[child.id])
        .await
        .unwrap();
    database
        .add_item_categories(items[3].id, 1, &[child.id])
        .await
        .unwrap();
    let filtered = payload(
        get(format!("{path}?category_id={}&limit=1", root.id))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(filtered["items"].as_array().unwrap().len(), 1);
    assert_eq!(filtered["items"][0]["name"], "100% Casque");
    assert_eq!(filtered["next_cursor"], "3");
    let next = payload(
        get(format!("{path}?category_id={}&limit=1&after=3", root.id))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(next["items"][0]["name"], "casque");
    assert!(next["next_cursor"].is_null());
    let storage_room = database
        .create_location(space.id, None, "Réserve")
        .await
        .unwrap();
    let shelf = database
        .create_location(space.id, Some(storage_room.id), "Étagère")
        .await
        .unwrap();
    database
        .move_item_to_location(items[2].id, Some(shelf.id), 2, account_id)
        .await
        .unwrap();
    let by_room = payload(
        get(format!("{path}?location_id={}", storage_room.id))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(by_room["items"].as_array().unwrap().len(), 1);
    assert_eq!(by_room["items"][0]["name"], "100% Casque");
    let combined = payload(
        get(format!(
            "{path}?location_id={}&category_id={}&q=Casque",
            storage_room.id, root.id
        ))
        .await
        .unwrap(),
    )
    .await;
    assert_eq!(combined["items"].as_array().unwrap().len(), 1);
    assert_eq!(combined["items"][0]["name"], "100% Casque");
    assert_eq!(
        get(format!("{path}?location_id={}", Uuid::now_v7()))
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(format!("{path}?location_id=invalid"))
            .await
            .unwrap()
            .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        get(format!("{path}?category_id={}", Uuid::now_v7()))
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(format!("{path}?category_id=invalid"))
            .await
            .unwrap()
            .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );

    for suffix in ["?limit=0", "?limit=101", "?after=not-a-number"] {
        assert_eq!(
            get(format!("{path}{suffix}")).await.unwrap().status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    assert_eq!(
        get(format!("{path}?q={}", "x".repeat(101)))
            .await
            .unwrap()
            .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let anonymous = router
        .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
}
