//! Real MariaDB and local-storage checks. Requires `COLLECTIONOPS_TEST_DATABASE_URL`.

use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode, header},
    response::Response,
};
use collectionops_backend::{
    BootstrapAdmin, Database, DocumentStore, PasswordService, Permission, SESSION_TOKEN_HEADER,
    app_with_database_mail_documents,
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const PASSWORD: &str = "correct horse battery staple";
const PDF: &[u8] = b"%PDF-1.7\n1 0 obj\n<<>>\nendobj\n%%EOF\n";

async fn send(
    router: &Router,
    method: Method,
    path: &str,
    token: Option<&str>,
    body: &[u8],
    media_type: Option<&str>,
) -> Response {
    let mut request = Request::builder().method(method).uri(path);
    if let Some(token) = token {
        request = request.header(SESSION_TOKEN_HEADER, token);
    }
    if let Some(media_type) = media_type {
        request = request
            .header(header::CONTENT_TYPE, media_type)
            .header("x-document-name", "notice%20%C3%A9t%C3%A9.pdf");
    }
    router
        .clone()
        .oneshot(request.body(Body::from(body.to_vec())).unwrap())
        .await
        .unwrap()
}

async fn json_body(response: Response) -> Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

async fn login(router: &Router, email: &str) -> String {
    let response = send(
        router,
        Method::POST,
        "/api/v1/sessions",
        None,
        json!({"email":email,"password":PASSWORD})
            .to_string()
            .as_bytes(),
        Some("application/json"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    json_body(response).await["token"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn documents_are_space_scoped_and_remain_at_source_after_transfer() {
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
    let owner = Uuid::parse_str(std::str::from_utf8(&owner_raw).unwrap()).unwrap();
    let reader = Uuid::now_v7();
    sqlx::query("INSERT INTO accounts (id, display_name, email, password_hash) SELECT ?, 'Reader', 'reader@example.com', password_hash FROM accounts WHERE email = ?")
        .bind(reader.to_string()).bind("owner@example.com").execute(db.pool()).await.unwrap();
    let source = db.create_space("Source", owner).await.unwrap();
    let target = db.create_space("Destination", owner).await.unwrap();
    db.add_member(source.id, reader, &[Permission::DocumentsRead])
        .await
        .unwrap();
    let item = db
        .create_item(source.id, "Mirage 2000B", owner)
        .await
        .unwrap();
    let root =
        std::env::temp_dir().join(format!("collectionops-documents-test-{}", Uuid::now_v7()));
    let store = DocumentStore::new(&root).unwrap();
    let router = app_with_database_mail_documents(db.clone(), None, Some(store));
    let owner_token = login(&router, "owner@example.com").await;
    let reader_token = login(&router, "reader@example.com").await;
    let source_path = format!("/api/v1/spaces/{}/items/{}/documents", source.id, item.id);
    let target_path = format!("/api/v1/spaces/{}/items/{}/documents", target.id, item.id);

    assert_eq!(
        send(&router, Method::GET, &source_path, None, b"", None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send(
            &router,
            Method::POST,
            &source_path,
            Some(&reader_token),
            PDF,
            Some("application/pdf")
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(
            &router,
            Method::POST,
            &source_path,
            Some(&owner_token),
            PDF,
            Some("image/png")
        )
        .await
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let created = send(
        &router,
        Method::POST,
        &source_path,
        Some(&owner_token),
        PDF,
        Some("application/pdf"),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let created = json_body(created).await;
    assert_eq!(created["original_name"], "notice été.pdf");
    assert_eq!(created["byte_size"], PDF.len());
    let document_id = created["id"].as_str().unwrap();
    let file_path = format!("{source_path}/{document_id}");
    let listed = send(
        &router,
        Method::GET,
        &source_path,
        Some(&reader_token),
        b"",
        None,
    )
    .await;
    assert_eq!(listed.status(), StatusCode::OK);
    assert_eq!(
        json_body(listed).await["documents"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let downloaded = send(
        &router,
        Method::GET,
        &file_path,
        Some(&reader_token),
        b"",
        None,
    )
    .await;
    assert_eq!(downloaded.status(), StatusCode::OK);
    assert_eq!(
        downloaded.into_body().collect().await.unwrap().to_bytes(),
        PDF
    );
    assert_eq!(
        send(
            &router,
            Method::GET,
            &target_path,
            Some(&reader_token),
            b"",
            None
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );

    db.transfer_item(item.id, target.id, item.revision, owner)
        .await
        .unwrap();
    let historic = send(
        &router,
        Method::GET,
        &source_path,
        Some(&reader_token),
        b"",
        None,
    )
    .await;
    assert_eq!(
        json_body(historic).await["documents"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let destination = send(
        &router,
        Method::GET,
        &target_path,
        Some(&owner_token),
        b"",
        None,
    )
    .await;
    assert!(
        json_body(destination).await["documents"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        send(
            &router,
            Method::POST,
            &source_path,
            Some(&owner_token),
            PDF,
            Some("application/pdf")
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );

    assert_eq!(
        send(
            &router,
            Method::DELETE,
            &file_path,
            Some(&reader_token),
            b"",
            None
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(
            &router,
            Method::DELETE,
            &file_path,
            Some(&owner_token),
            b"",
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
            &file_path,
            Some(&owner_token),
            b"",
            None
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    let quota_item = db
        .create_item(source.id, "Documentation", owner)
        .await
        .unwrap();
    let quota_path = format!(
        "/api/v1/spaces/{}/items/{}/documents",
        source.id, quota_item.id
    );
    for _ in 0..50 {
        assert_eq!(
            send(
                &router,
                Method::POST,
                &quota_path,
                Some(&owner_token),
                PDF,
                Some("application/pdf")
            )
            .await
            .status(),
            StatusCode::CREATED
        );
    }
    assert_eq!(
        send(
            &router,
            Method::POST,
            &quota_path,
            Some(&owner_token),
            PDF,
            Some("application/pdf")
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    // The file is retained until a purge policy is approved. Only the explicitly named temp
    // directory is removed, never a workspace or a user-provided directory.
    assert_eq!(
        root.parent().unwrap().canonicalize().unwrap(),
        std::env::temp_dir().canonicalize().unwrap()
    );
    std::fs::remove_dir_all(&root).unwrap();
}
