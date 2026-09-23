//! HTTP contract for item archiving, the trash, restoration and the state filter.

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
async fn item_states_are_protected_audited_and_filterable() {
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
    let space = database.create_space("Collection", owner_id).await.unwrap();
    let item = database
        .create_item(space.id, "Objet", owner_id)
        .await
        .unwrap();
    let archive_path = format!("/api/v1/items/{}/archive", item.id);
    let trash_path = format!("/api/v1/items/{}/trash", item.id);
    let restore_path = format!("/api/v1/items/{}/restore", item.id);
    let state_events_path = format!("/api/v1/items/{}/state-events", item.id);
    let item_path = format!("/api/v1/items/{}", item.id);
    let list_path = format!("/api/v1/spaces/{}/items", space.id);

    assert_eq!(
        send(&router, Method::GET, &state_events_path, None, None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send(
            &router,
            Method::GET,
            &state_events_path,
            Some(&other_token),
            None
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );

    assert_eq!(
        send(
            &router,
            Method::POST,
            &archive_path,
            None,
            Some(json!({"expected_revision":"1"}))
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send(
            &router,
            Method::POST,
            &archive_path,
            Some(&other_token),
            Some(json!({"expected_revision":"1"}))
        )
        .await
        .status(),
        StatusCode::NOT_FOUND,
        "a non-member must not learn that the item exists"
    );
    assert_eq!(
        send(
            &router,
            Method::POST,
            &archive_path,
            Some(&owner_token),
            Some(json!({"expected_revision":"zero"}))
        )
        .await
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        send(&router, Method::GET, &list_path, Some(&owner_token), None)
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        send(
            &router,
            Method::GET,
            &format!("{list_path}?state=weird"),
            Some(&owner_token),
            None
        )
        .await
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );

    let archived = send(
        &router,
        Method::POST,
        &archive_path,
        Some(&owner_token),
        Some(json!({"expected_revision":"1"})),
    )
    .await;
    assert_eq!(archived.status(), StatusCode::OK);
    let archived = json_body(archived).await;
    assert_eq!(archived["state"], "archived");
    assert_eq!(archived["revision"], "2");

    // The default view hides the archived item; asking for all states brings it back.
    let visible = send(&router, Method::GET, &list_path, Some(&owner_token), None).await;
    let visible = json_body(visible).await;
    assert_eq!(visible["items"].as_array().unwrap().len(), 0);
    let all = send(
        &router,
        Method::GET,
        &format!("{list_path}?state=all"),
        Some(&owner_token),
        None,
    )
    .await;
    let all = json_body(all).await;
    assert_eq!(all["items"].as_array().unwrap().len(), 1);

    let trashed = send(
        &router,
        Method::POST,
        &trash_path,
        Some(&owner_token),
        Some(json!({"expected_revision":"2"})),
    )
    .await;
    assert_eq!(trashed.status(), StatusCode::OK);
    let trashed = json_body(trashed).await;
    assert_eq!(trashed["state"], "trashed");
    assert_eq!(trashed["revision"], "3");

    // A trashed item is read-only: renaming must be refused, not silently accepted.
    let rename = send(
        &router,
        Method::PATCH,
        &item_path,
        Some(&owner_token),
        Some(json!({"name":"Nouveau","expected_revision":"3"})),
    )
    .await;
    assert_eq!(rename.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(json_body(rename).await["code"], "invalid_state");

    let restored = send(
        &router,
        Method::POST,
        &restore_path,
        Some(&owner_token),
        Some(json!({"expected_revision":"3"})),
    )
    .await;
    assert_eq!(restored.status(), StatusCode::OK);
    let restored = json_body(restored).await;
    assert_eq!(restored["state"], "active");
    assert_eq!(restored["revision"], "4");

    // Restoring an already active item is not an allowed transition.
    let noop = send(
        &router,
        Method::POST,
        &restore_path,
        Some(&owner_token),
        Some(json!({"expected_revision":"4"})),
    )
    .await;
    assert_eq!(noop.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(json_body(noop).await["code"], "invalid_transition");

    let events = send(
        &router,
        Method::GET,
        &state_events_path,
        Some(&owner_token),
        None,
    )
    .await;
    assert_eq!(events.status(), StatusCode::OK);
    let events = json_body(events).await;
    let events = events["events"].as_array().unwrap();
    assert_eq!(
        events.len(),
        3,
        "refused transitions must not create audit rows"
    );
    assert_eq!(events[0]["state_before"], "trashed");
    assert_eq!(events[0]["state_after"], "active");
    assert_eq!(events[0]["actor_account_id"], owner_id.to_string());

    // A member with read-only access may see the item but not change its state.
    database
        .add_member(space.id, outsider, &[Permission::CollectionsRead])
        .await
        .unwrap();
    assert_eq!(
        send(&router, Method::GET, &item_path, Some(&other_token), None)
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        send(
            &router,
            Method::POST,
            &archive_path,
            Some(&other_token),
            Some(json!({"expected_revision":"4"}))
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(
            &router,
            Method::GET,
            &state_events_path,
            Some(&other_token),
            None
        )
        .await
        .status(),
        StatusCode::NOT_FOUND,
        "read-only members cannot inspect the sensitive actor audit"
    );

    // Granting write lets the same member archive, and the audit row names the member.
    database
        .set_member_permissions(
            space.id,
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
            &archive_path,
            Some(&other_token),
            Some(json!({"expected_revision":"4"}))
        )
        .await
        .status(),
        StatusCode::OK
    );
    let actor: Vec<u8> = sqlx::query_scalar(
        "SELECT actor_account_id FROM inventory_item_audit_events WHERE item_id = ? ORDER BY created_at DESC LIMIT 1",
    )
    .bind(item.id.to_string())
    .fetch_one(database.pool())
    .await
    .unwrap();
    assert_eq!(String::from_utf8(actor).unwrap(), outsider.to_string());

    let events = send(
        &router,
        Method::GET,
        &state_events_path,
        Some(&owner_token),
        None,
    )
    .await;
    let events = json_body(events).await;
    assert_eq!(events["events"].as_array().unwrap().len(), 4);
    assert_eq!(
        events["events"][0]["actor_account_id"],
        outsider.to_string()
    );
    assert_eq!(events["events"][0]["actor_display_name"], "Other");

    // A transfer does not grant the destination space access to prior-space audit events.
    let destination = database
        .create_space("Destination", owner_id)
        .await
        .unwrap();
    database
        .transfer_item(item.id, destination.id, 5, owner_id)
        .await
        .unwrap();
    let events = send(
        &router,
        Method::GET,
        &state_events_path,
        Some(&owner_token),
        None,
    )
    .await;
    assert_eq!(events.status(), StatusCode::OK);
    assert_eq!(
        json_body(events).await["events"].as_array().unwrap().len(),
        0
    );
}
