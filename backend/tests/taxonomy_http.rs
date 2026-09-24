//! Category hierarchy and category-owned field definitions through the authenticated API.

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
                .body(body.map_or_else(Body::empty, |v| Body::from(v.to_string())))
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
async fn nested_categories_and_fields_stay_inside_their_space() {
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
    let other_space = db.create_space("Autre", owner_id).await.unwrap();
    db.add_member(space.id, reader_id, &[Permission::CollectionsRead])
        .await
        .unwrap();
    let router = collectionops_backend::app_with_database(db.clone());
    let owner_token = login(&router, "owner@example.com").await;
    let reader_token = login(&router, "reader@example.com").await;
    let categories_path = format!("/api/v1/spaces/{}/categories", space.id);

    assert_eq!(
        send(&router, Method::GET, &categories_path, None, None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let root = send(
        &router,
        Method::POST,
        &categories_path,
        Some(&owner_token),
        Some(json!({"name":"Armes", "parent_id":null})),
    )
    .await;
    assert_eq!(root.status(), StatusCode::CREATED);
    let root = body_json(root).await;
    let root_id = Uuid::parse_str(root["id"].as_str().unwrap()).unwrap();
    assert!(root["parent_id"].is_null());
    assert_eq!(
        send(
            &router,
            Method::POST,
            &categories_path,
            Some(&owner_token),
            Some(json!({"name":"armes", "parent_id":null}))
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );

    let child = send(
        &router,
        Method::POST,
        &categories_path,
        Some(&owner_token),
        Some(json!({"name":"Casques", "parent_id":root_id})),
    )
    .await;
    assert_eq!(child.status(), StatusCode::CREATED);
    let child = body_json(child).await;
    assert_eq!(child["parent_id"], root_id.to_string());
    let child_id = Uuid::parse_str(child["id"].as_str().unwrap()).unwrap();
    let grandchild = send(
        &router,
        Method::POST,
        &categories_path,
        Some(&owner_token),
        Some(json!({"name":"Casques M1", "parent_id":child_id})),
    )
    .await;
    assert_eq!(grandchild.status(), StatusCode::CREATED);
    assert_eq!(
        send(
            &router,
            Method::POST,
            &categories_path,
            Some(&reader_token),
            Some(json!({"name":"Interdit", "parent_id":null}))
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let categories = body_json(
        send(
            &router,
            Method::GET,
            &categories_path,
            Some(&reader_token),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(categories["categories"].as_array().unwrap().len(), 3);

    let other_path = format!("/api/v1/spaces/{}/categories", other_space.id);
    assert_eq!(
        send(
            &router,
            Method::POST,
            &other_path,
            Some(&owner_token),
            Some(json!({"name":"Invalid", "parent_id":root_id}))
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );

    let fields_path = format!("{categories_path}/{root_id}/fields");
    let field = send(
        &router,
        Method::POST,
        &fields_path,
        Some(&owner_token),
        Some(json!({"name":"Fabricant", "value_type":"text"})),
    )
    .await;
    assert_eq!(field.status(), StatusCode::CREATED);
    let field = body_json(field).await;
    assert_eq!(field["value_type"], "text");
    assert_eq!(
        send(
            &router,
            Method::POST,
            &fields_path,
            Some(&owner_token),
            Some(json!({"name":"fabricant", "value_type":"date"}))
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        send(
            &router,
            Method::POST,
            &fields_path,
            Some(&owner_token),
            Some(json!({"name":"Prix", "value_type":"currency"}))
        )
        .await
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        send(
            &router,
            Method::POST,
            &fields_path,
            Some(&reader_token),
            Some(json!({"name":"Date", "value_type":"date"}))
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let fields = body_json(
        send(
            &router,
            Method::GET,
            &fields_path,
            Some(&reader_token),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(fields["fields"].as_array().unwrap().len(), 1);
    let other_fields_path = format!("{other_path}/{root_id}/fields");
    assert_eq!(
        send(
            &router,
            Method::GET,
            &other_fields_path,
            Some(&owner_token),
            None
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );

    let sibling = db
        .create_category(space.id, Some(root_id), "Uniformes")
        .await
        .unwrap();
    let number_field = db
        .create_category_field(
            space.id,
            child_id,
            "Taille",
            collectionops_backend::FieldType::Number,
        )
        .await
        .unwrap();
    let date_field = db
        .create_category_field(
            space.id,
            sibling.id,
            "Date",
            collectionops_backend::FieldType::Date,
        )
        .await
        .unwrap();
    let item = db.create_item(space.id, "Objet", owner_id).await.unwrap();
    let item_categories_path = format!("/api/v1/items/{}/categories", item.id);
    let attach = send(
        &router,
        Method::POST,
        &item_categories_path,
        Some(&owner_token),
        Some(json!({"category_ids":[child_id, sibling.id], "expected_revision":"1"})),
    )
    .await;
    assert_eq!(attach.status(), StatusCode::OK);
    assert_eq!(body_json(attach).await["revision"], "2");
    let item_fields_path = format!("/api/v1/items/{}/fields", item.id);
    let effective = body_json(
        send(
            &router,
            Method::GET,
            &item_fields_path,
            Some(&reader_token),
            None,
        )
        .await,
    )
    .await;
    let fields = effective["fields"].as_array().unwrap();
    assert_eq!(
        fields.len(),
        3,
        "the common ancestor's field appears only once"
    );
    assert!(fields.iter().any(|field| field["name"] == "Fabricant"));
    assert!(fields.iter().any(|field| field["name"] == "Taille"));
    assert!(fields.iter().any(|field| field["name"] == "Date"));
    let text_id = Uuid::parse_str(field["id"].as_str().unwrap()).unwrap();
    let text_path = format!("{item_fields_path}/{text_id}");
    assert_eq!(
        send(
            &router,
            Method::PUT,
            &text_path,
            Some(&reader_token),
            Some(json!({"value":"US", "expected_revision":"2"}))
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let written = send(
        &router,
        Method::PUT,
        &text_path,
        Some(&owner_token),
        Some(json!({"value":"US", "expected_revision":"2"})),
    )
    .await;
    assert_eq!(written.status(), StatusCode::OK);
    assert_eq!(body_json(written).await["revision"], "3");
    let number_path = format!("{item_fields_path}/{}", number_field.id);
    assert_eq!(
        send(
            &router,
            Method::PUT,
            &number_path,
            Some(&owner_token),
            Some(json!({"value":"not a number", "expected_revision":"3"}))
        )
        .await
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let written = send(
        &router,
        Method::PUT,
        &number_path,
        Some(&owner_token),
        Some(json!({"value":"12.5", "expected_revision":"3"})),
    )
    .await;
    assert_eq!(written.status(), StatusCode::OK);
    assert_eq!(body_json(written).await["revision"], "4");
    let date_path = format!("{item_fields_path}/{}", date_field.id);
    let written = send(
        &router,
        Method::PUT,
        &date_path,
        Some(&owner_token),
        Some(json!({"value":"2020-04-15", "expected_revision":"4"})),
    )
    .await;
    assert_eq!(written.status(), StatusCode::OK);
    assert_eq!(body_json(written).await["revision"], "5");
    let current = body_json(
        send(
            &router,
            Method::GET,
            &item_fields_path,
            Some(&owner_token),
            None,
        )
        .await,
    )
    .await;
    let fields = current["fields"].as_array().unwrap();
    assert!(
        fields
            .iter()
            .any(|field| field["name"] == "Fabricant" && field["value"] == "US")
    );
    assert!(fields.iter().any(|field| {
        field["name"] == "Taille"
            && field["value"]
                .as_str()
                .is_some_and(|value| value.starts_with("12.5"))
    }));
    assert!(
        fields
            .iter()
            .any(|field| field["name"] == "Date" && field["value"] == "2020-04-15")
    );
    assert_eq!(
        send(
            &router,
            Method::PUT,
            &text_path,
            Some(&owner_token),
            Some(json!({"value":"Ecrasement", "expected_revision":"2"}))
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    let wrong_category = db
        .create_category(other_space.id, None, "Autre catégorie")
        .await
        .unwrap();
    assert_eq!(
        send(
            &router,
            Method::POST,
            &item_categories_path,
            Some(&owner_token),
            Some(json!({"category_ids":[wrong_category.id], "expected_revision":"5"}))
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );

    let transfer_path = format!("/api/v1/items/{}/transfers", item.id);
    assert_eq!(
        send(
            &router,
            Method::POST,
            &transfer_path,
            Some(&owner_token),
            Some(json!({"destination_space_id":other_space.id, "expected_revision":"5"}))
        )
        .await
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        send(
            &router,
            Method::POST,
            &transfer_path,
            Some(&owner_token),
            Some(
                json!({"destination_space_id":other_space.id, "expected_revision":"5",
            "destination_category_ids":[root_id]})
            )
        )
        .await
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let moved = send(
        &router,
        Method::POST,
        &transfer_path,
        Some(&owner_token),
        Some(
            json!({"destination_space_id":other_space.id, "expected_revision":"5",
            "destination_category_ids":[wrong_category.id]}),
        ),
    )
    .await;
    assert_eq!(moved.status(), StatusCode::OK);
    let moved = body_json(moved).await;
    assert_eq!(moved["item"]["revision"], "6");
    let categories = body_json(
        send(
            &router,
            Method::GET,
            &item_categories_path,
            Some(&owner_token),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(categories["categories"].as_array().unwrap().len(), 1);
    assert_eq!(
        categories["categories"][0]["category_id"],
        wrong_category.id.to_string()
    );
    let fields = body_json(
        send(
            &router,
            Method::GET,
            &item_fields_path,
            Some(&owner_token),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(fields["fields"].as_array().unwrap().len(), 0);
    let retained: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM item_custom_field_values")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(
        retained, 3,
        "old category values remain as history after transfer"
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn replacing_categories_preserves_shared_values_and_ends_removed_assignments() {
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
    let root = db.create_category(space.id, None, "Armes").await.unwrap();
    let old = db
        .create_category(space.id, Some(root.id), "Casques")
        .await
        .unwrap();
    let new = db
        .create_category(space.id, Some(root.id), "Uniformes")
        .await
        .unwrap();
    let foreign = db.create_category(other.id, None, "Autre").await.unwrap();
    let shared_field = db
        .create_category_field(
            space.id,
            root.id,
            "Fabricant",
            collectionops_backend::FieldType::Text,
        )
        .await
        .unwrap();
    let old_field = db
        .create_category_field(
            space.id,
            old.id,
            "Taille",
            collectionops_backend::FieldType::Number,
        )
        .await
        .unwrap();
    let item = db.create_item(space.id, "Objet", owner_id).await.unwrap();
    assert_eq!(
        db.add_item_categories(item.id, 1, &[old.id]).await.unwrap(),
        2
    );
    assert_eq!(
        db.set_item_field_value(item.id, shared_field.id, 2, "US")
            .await
            .unwrap(),
        3
    );
    assert_eq!(
        db.set_item_field_value(item.id, old_field.id, 3, "12.5")
            .await
            .unwrap(),
        4
    );
    let router = collectionops_backend::app_with_database(db.clone());
    let owner_token = login(&router, "owner@example.com").await;
    let reader_token = login(&router, "reader@example.com").await;
    let path = format!("/api/v1/items/{}/categories", item.id);

    assert_eq!(
        send(
            &router,
            Method::PUT,
            &path,
            Some(&reader_token),
            Some(json!({"category_ids":[new.id], "expected_revision":"4"}))
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(
            &router,
            Method::PUT,
            &path,
            Some(&owner_token),
            Some(json!({"category_ids":[foreign.id], "expected_revision":"4"}))
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
            Some(json!({"category_ids":[new.id], "expected_revision":"3"}))
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
        Some(json!({"category_ids":[new.id], "expected_revision":"4"})),
    )
    .await;
    assert_eq!(replaced.status(), StatusCode::OK);
    let replaced = body_json(replaced).await;
    assert_eq!(replaced["revision"], "5");
    assert_eq!(replaced["categories"][0]["category_id"], new.id.to_string());
    let fields = db.effective_item_fields(item.id, space.id).await.unwrap();
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].id, shared_field.id);
    assert_eq!(fields[0].value.as_deref(), Some("US"));
    let ended: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM item_category_assignments WHERE item_id = ? AND ended_at IS NOT NULL",
    )
    .bind(item.id.to_string())
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(ended, 1);
    let retained: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM item_custom_field_values")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(retained, 3, "old values remain and shared value is copied");

    let no_op = body_json(
        send(
            &router,
            Method::PUT,
            &path,
            Some(&owner_token),
            Some(json!({"category_ids":[new.id], "expected_revision":"5"})),
        )
        .await,
    )
    .await;
    assert_eq!(no_op["revision"], "5");
    let emptied = body_json(
        send(
            &router,
            Method::PUT,
            &path,
            Some(&owner_token),
            Some(json!({"category_ids":[], "expected_revision":"5"})),
        )
        .await,
    )
    .await;
    assert_eq!(emptied["revision"], "6");
    assert_eq!(emptied["categories"].as_array().unwrap().len(), 0);
    assert!(
        db.effective_item_fields(item.id, space.id)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn categories_and_fields_can_be_renamed_without_changing_identity() {
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
    let other_space = db.create_space("Autre", owner_id).await.unwrap();
    db.add_member(space.id, reader_id, &[Permission::CollectionsRead])
        .await
        .unwrap();
    let router = collectionops_backend::app_with_database(db.clone());
    let owner_token = login(&router, "owner@example.com").await;
    let reader_token = login(&router, "reader@example.com").await;
    let categories_path = format!("/api/v1/spaces/{}/categories", space.id);

    let root = db.create_category(space.id, None, "Armes").await.unwrap();
    let child = db
        .create_category(space.id, Some(root.id), "Casques")
        .await
        .unwrap();
    let sibling = db
        .create_category(space.id, Some(root.id), "Uniformes")
        .await
        .unwrap();
    let alien = db
        .create_category(other_space.id, None, "Alien")
        .await
        .unwrap();

    let category_path = format!("{categories_path}/{}", child.id);
    // A blank name is refused and never reaches the database.
    assert_eq!(
        send(
            &router,
            Method::PATCH,
            &category_path,
            Some(&owner_token),
            Some(json!({"name":"   "}))
        )
        .await
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        send(
            &router,
            Method::PATCH,
            &category_path,
            Some(&reader_token),
            Some(json!({"name":"Interdit"}))
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    // A sibling already uses the requested name.
    assert_eq!(
        send(
            &router,
            Method::PATCH,
            &category_path,
            Some(&owner_token),
            Some(json!({"name":sibling.name}))
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    let renamed = send(
        &router,
        Method::PATCH,
        &category_path,
        Some(&owner_token),
        Some(json!({"name":"Casques Lourds"})),
    )
    .await;
    assert_eq!(renamed.status(), StatusCode::OK);
    let renamed = body_json(renamed).await;
    assert_eq!(renamed["id"], child.id.to_string());
    assert_eq!(renamed["parent_id"], root.id.to_string());
    assert_eq!(renamed["name"], "Casques Lourds");
    // An identical rename is accepted and keeps the identifier.
    let no_op = body_json(
        send(
            &router,
            Method::PATCH,
            &category_path,
            Some(&owner_token),
            Some(json!({"name":"Casques Lourds"})),
        )
        .await,
    )
    .await;
    assert_eq!(no_op["id"], child.id.to_string());
    assert_eq!(no_op["name"], "Casques Lourds");
    // A category from another space is invisible even with the right identifier.
    assert_eq!(
        send(
            &router,
            Method::PATCH,
            &format!("{categories_path}/{}", alien.id),
            Some(&owner_token),
            Some(json!({"name":"Fuite"}))
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );

    let fields_path = format!("{categories_path}/{}/fields", root.id);
    let factory = body_json(
        send(
            &router,
            Method::POST,
            &fields_path,
            Some(&owner_token),
            Some(json!({"name":"Fabricant", "value_type":"text"})),
        )
        .await,
    )
    .await;
    let factory_id = Uuid::parse_str(factory["id"].as_str().unwrap()).unwrap();
    let material = body_json(
        send(
            &router,
            Method::POST,
            &fields_path,
            Some(&owner_token),
            Some(json!({"name":"Matiere", "value_type":"text"})),
        )
        .await,
    )
    .await;
    let material_id = Uuid::parse_str(material["id"].as_str().unwrap()).unwrap();

    let field_path = format!("{fields_path}/{material_id}");
    assert_eq!(
        send(
            &router,
            Method::PATCH,
            &field_path,
            Some(&owner_token),
            Some(json!({"name":factory["name"]}))
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        send(
            &router,
            Method::PATCH,
            &field_path,
            Some(&reader_token),
            Some(json!({"name":"Interdit"}))
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let renamed_field = body_json(
        send(
            &router,
            Method::PATCH,
            &field_path,
            Some(&owner_token),
            Some(json!({"name":"Materiau"})),
        )
        .await,
    )
    .await;
    assert_eq!(renamed_field["id"], material_id.to_string());
    assert_eq!(renamed_field["category_id"], root.id.to_string());
    assert_eq!(renamed_field["name"], "Materiau");
    assert_eq!(
        renamed_field["value_type"], "text",
        "renaming keeps the fixed value type"
    );
    // A field addressed through the wrong category is not found.
    assert_eq!(
        send(
            &router,
            Method::PATCH,
            &format!("{categories_path}/{}/fields/{material_id}", child.id),
            Some(&owner_token),
            Some(json!({"name":"Deracine"}))
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    // Renaming an unknown field is a 404, not a silent success.
    assert_eq!(
        send(
            &router,
            Method::PATCH,
            &format!("{fields_path}/{}", Uuid::now_v7()),
            Some(&owner_token),
            Some(json!({"name":"Fantome"}))
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );

    // Stored values still point at the same field after its rename.
    let item = db.create_item(space.id, "Casque", owner_id).await.unwrap();
    assert_eq!(
        send(
            &router,
            Method::POST,
            &format!("/api/v1/items/{}/categories", item.id),
            Some(&owner_token),
            Some(json!({"category_ids":[root.id], "expected_revision":"1"})),
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        send(
            &router,
            Method::PUT,
            &format!("/api/v1/items/{}/fields/{factory_id}", item.id),
            Some(&owner_token),
            Some(json!({"value":"Schuberth", "expected_revision":"2"})),
        )
        .await
        .status(),
        StatusCode::OK
    );
    let fields = db.effective_item_fields(item.id, space.id).await.unwrap();
    assert_eq!(fields.len(), 2);
    assert_eq!(
        fields
            .iter()
            .find(|field| field.id == factory_id)
            .and_then(|field| field.value.clone())
            .as_deref(),
        Some("Schuberth")
    );
    assert!(
        fields.iter().any(|field| field.name == "Materiau"),
        "the renamed definition is what the item now displays"
    );
}
