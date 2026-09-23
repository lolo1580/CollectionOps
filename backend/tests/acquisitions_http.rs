//! Acquisition prospects remain space-scoped and contain no financial amounts.

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
    sqlx::query("INSERT INTO accounts (id, display_name, email, password_hash) SELECT ?, ?, ?, password_hash FROM accounts WHERE email = 'owner@example.com'")
        .bind(id.to_string()).bind(email).bind(email).execute(db.pool()).await.unwrap();
    id
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn wishes_vendors_and_offers_require_explicit_acquisition_rights() {
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
    let owner_raw: Vec<u8> =
        sqlx::query_scalar("SELECT id FROM accounts WHERE email = 'owner@example.com'")
            .fetch_one(db.pool())
            .await
            .unwrap();
    let owner_id = Uuid::parse_str(std::str::from_utf8(&owner_raw).unwrap()).unwrap();
    let reader = insert_account(&db, "reader@example.com").await;
    let writer = insert_account(&db, "writer@example.com").await;
    let inventory_only = insert_account(&db, "inventory@example.com").await;
    let space = db.create_space("Collection", owner_id).await.unwrap();
    let other = db.create_space("Autre", owner_id).await.unwrap();
    db.add_member(space.id, reader, &[Permission::AcquisitionsRead])
        .await
        .unwrap();
    db.add_member(
        space.id,
        writer,
        &[Permission::AcquisitionsRead, Permission::AcquisitionsWrite],
    )
    .await
    .unwrap();
    db.add_member(space.id, inventory_only, &[Permission::CollectionsRead])
        .await
        .unwrap();
    let router = collectionops_backend::app_with_database(db.clone());
    let owner_token = login(&router, "owner@example.com").await;
    let reader_token = login(&router, "reader@example.com").await;
    let writer_token = login(&router, "writer@example.com").await;
    let inventory_token = login(&router, "inventory@example.com").await;
    let wishes_path = format!("/api/v1/spaces/{}/wishes", space.id);
    let vendors_path = format!("/api/v1/spaces/{}/vendors", space.id);

    let reader_spaces = send(
        &router,
        Method::GET,
        "/api/v1/spaces",
        Some(&reader_token),
        None,
    )
    .await;
    assert_eq!(reader_spaces.status(), StatusCode::OK);
    assert_eq!(
        body_json(reader_spaces).await["spaces"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let rights = send(
        &router,
        Method::GET,
        &format!("/api/v1/spaces/{}/my-permissions", space.id),
        Some(&reader_token),
        None,
    )
    .await;
    assert_eq!(rights.status(), StatusCode::OK);
    assert_eq!(
        body_json(rights).await["permissions"],
        json!(["acquisitions_read"])
    );
    assert_eq!(
        send(
            &router,
            Method::GET,
            &wishes_path,
            Some(&inventory_token),
            None
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(
            &router,
            Method::POST,
            &wishes_path,
            Some(&reader_token),
            Some(json!({"title":"Interdit"}))
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(&router, Method::GET, &wishes_path, None, None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );

    let wish = send(
        &router,
        Method::POST,
        &wishes_path,
        Some(&writer_token),
        Some(json!({"title":"  Casque M1  ","search_notes":" État correct "})),
    )
    .await;
    assert_eq!(wish.status(), StatusCode::CREATED);
    let wish = body_json(wish).await;
    assert_eq!(wish["title"], "Casque M1");
    assert_eq!(wish["search_notes"], "État correct");
    assert!(wish.get("price").is_none());
    let wish_id = wish["id"].as_str().unwrap();
    let vendor = send(
        &router,
        Method::POST,
        &vendors_path,
        Some(&writer_token),
        Some(json!({"name":"Marchand A","website_url":"https://example.org/catalogue"})),
    )
    .await;
    assert_eq!(vendor.status(), StatusCode::CREATED);
    let vendor = body_json(vendor).await;
    let vendor_id = vendor["id"].as_str().unwrap();
    assert_eq!(
        send(
            &router,
            Method::POST,
            &vendors_path,
            Some(&writer_token),
            Some(json!({"name":"Marchand A"}))
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        send(
            &router,
            Method::POST,
            &vendors_path,
            Some(&writer_token),
            Some(json!({"name":"Lien mauvais","website_url":"file:///etc/passwd"}))
        )
        .await
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );

    let offers_path = format!("{wishes_path}/{wish_id}/offers");
    let foreign_vendor = db
        .create_vendor(other.id, "Marchand B", None)
        .await
        .unwrap();
    assert_eq!(
        send(
            &router,
            Method::POST,
            &offers_path,
            Some(&writer_token),
            Some(json!({"vendor_id":foreign_vendor.id,"title":"Offre externe"}))
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    let offer = send(
        &router,
        Method::POST,
        &offers_path,
        Some(&writer_token),
        Some(json!({"vendor_id":vendor_id,"title":"Casque original",
            "source_url":"https://example.org/item/42","notes":"Vérifier marquage"})),
    )
    .await;
    assert_eq!(offer.status(), StatusCode::CREATED);
    let offer = body_json(offer).await;
    assert_eq!(offer["wish_id"], wish_id);
    assert_eq!(offer["vendor_id"], vendor_id);
    assert!(offer.get("amount").is_none());
    let listed = send(
        &router,
        Method::GET,
        &offers_path,
        Some(&reader_token),
        None,
    )
    .await;
    assert_eq!(listed.status(), StatusCode::OK);
    assert_eq!(
        body_json(listed).await["offers"].as_array().unwrap().len(),
        1
    );
    assert_eq!(
        send(
            &router,
            Method::GET,
            &offers_path,
            Some(&inventory_token),
            None
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(
            &router,
            Method::GET,
            &format!("/api/v1/spaces/{}/wishes/{wish_id}/offers", other.id),
            Some(&owner_token),
            None
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
}
