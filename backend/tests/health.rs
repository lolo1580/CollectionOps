use std::collections::BTreeSet;

use axum::{
    Extension,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

async fn response_json(response: axum::response::Response) -> Value {
    let body = to_bytes(response.into_body(), 256 * 1024)
        .await
        .expect("response body must be readable");
    serde_json::from_slice(&body).expect("response must be valid JSON")
}

#[tokio::test]
async fn health_endpoint_returns_service_status() {
    let response = collectionops_backend::app()
        .oneshot(
            Request::builder()
                .uri("/api/v1/health")
                .body(Body::empty())
                .expect("request must be valid"),
        )
        .await
        .expect("router must answer");

    assert_eq!(response.status(), StatusCode::OK);

    let payload = response_json(response).await;

    assert_eq!(payload["status"], "ok");
    assert_eq!(payload["service"], "collectionops-backend");
}

#[tokio::test]
async fn openapi_document_exposes_health_contract() {
    let response = collectionops_backend::app()
        .oneshot(
            Request::builder()
                .uri("/api/openapi.json")
                .body(Body::empty())
                .expect("request must be valid"),
        )
        .await
        .expect("router must answer");

    assert_eq!(response.status(), StatusCode::OK);

    let payload = response_json(response).await;

    assert!(payload["paths"]["/api/v1/health"].is_object());
    assert!(payload["paths"]["/api/v1/health/live"].is_object());
    assert!(payload["paths"]["/api/v1/health/ready"].is_object());
    assert!(payload["paths"]["/api/v1/items/{item_id}/transfers"]["post"].is_object());
    assert!(payload["paths"]["/api/v1/items/{item_id}/transfers"]["get"].is_object());
    let create_parameters =
        payload["paths"]["/api/v1/spaces/{space_id}/items"]["post"]["parameters"]
            .as_array()
            .unwrap();
    assert!(create_parameters.iter().any(|parameter| {
        parameter["name"] == "Idempotency-Key" && parameter["in"] == "header"
    }));
}

#[tokio::test]
async fn readiness_endpoint_reports_ready_without_external_dependencies() {
    let response = collectionops_backend::app()
        .oneshot(
            Request::builder()
                .uri("/api/v1/health/ready")
                .body(Body::empty())
                .expect("request must be valid"),
        )
        .await
        .expect("router must answer");

    assert_eq!(response.status(), StatusCode::OK);
    let payload = response_json(response).await;
    assert_eq!(payload["status"], "ready");
}

#[tokio::test]
async fn readiness_tracks_the_database_without_affecting_liveness() {
    let Ok(url) = std::env::var("COLLECTIONOPS_TEST_DATABASE_URL") else {
        return;
    };
    let database = collectionops_backend::Database::connect_and_migrate(&url)
        .await
        .expect("test database must be available");
    let pool = database.pool().clone();
    let router = collectionops_backend::app_with_database(database);

    let ready = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/health/ready")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(ready.status(), StatusCode::OK);
    assert_eq!(response_json(ready).await["status"], "ready");

    pool.close().await;
    let unavailable = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/health/ready")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unavailable.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response_json(unavailable).await["status"], "not_ready");

    let live = router
        .oneshot(
            Request::builder()
                .uri("/api/v1/health/live")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(live.status(), StatusCode::OK);
    assert_eq!(response_json(live).await["status"], "alive");
}

#[tokio::test]
async fn unknown_route_returns_problem_details() {
    let response = collectionops_backend::app()
        .oneshot(
            Request::builder()
                .uri("/api/v1/unknown")
                .body(Body::empty())
                .expect("request must be valid"),
        )
        .await
        .expect("router must answer");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let payload = response_json(response).await;
    assert_eq!(payload["status"], 404);
    assert_eq!(payload["code"], "route_not_found");
    assert_eq!(payload["instance"], "/api/v1/unknown");
}

#[tokio::test]
async fn session_endpoint_denies_anonymous_requests() {
    let response = collectionops_backend::app()
        .oneshot(
            Request::builder()
                .uri("/api/v1/session")
                .body(Body::empty())
                .expect("request must be valid"),
        )
        .await
        .expect("router must answer");

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE),
        Some(&header::HeaderValue::from_static(
            "application/problem+json"
        ))
    );
    let payload = response_json(response).await;
    assert_eq!(payload["code"], "authentication_required");
}

#[tokio::test]
async fn session_endpoint_returns_injected_principal() {
    let subject = Uuid::now_v7();
    let principal = collectionops_backend::Principal {
        subject,
        display_name: "Collectionneur".to_owned(),
        roles: BTreeSet::from(["collector".to_owned()]),
        permissions: BTreeSet::from([
            collectionops_backend::Permission::CollectionsRead,
            collectionops_backend::Permission::CollectionsWrite,
        ]),
    };
    let response = collectionops_backend::app()
        .layer(Extension(principal))
        .oneshot(
            Request::builder()
                .uri("/api/v1/session")
                .body(Body::empty())
                .expect("request must be valid"),
        )
        .await
        .expect("router must answer");

    assert_eq!(response.status(), StatusCode::OK);
    let payload = response_json(response).await;
    assert_eq!(payload["principal"]["subject"], subject.to_string());
    assert_eq!(payload["principal"]["display_name"], "Collectionneur");
    assert_eq!(payload["principal"]["permissions"][0], "collections_read");
}

#[tokio::test]
async fn request_id_is_preserved_when_it_is_a_valid_uuid() {
    let request_id = Uuid::now_v7().to_string();
    let response = collectionops_backend::app()
        .oneshot(
            Request::builder()
                .uri("/api/v1/health/live")
                .header(collectionops_backend::REQUEST_ID_HEADER, &request_id)
                .body(Body::empty())
                .expect("request must be valid"),
        )
        .await
        .expect("router must answer");

    assert_eq!(
        response
            .headers()
            .get(collectionops_backend::REQUEST_ID_HEADER)
            .expect("response must contain a request ID")
            .to_str()
            .expect("request ID must be text"),
        request_id.as_str()
    );
    assert_eq!(
        response.headers().get(header::X_CONTENT_TYPE_OPTIONS),
        Some(&header::HeaderValue::from_static("nosniff"))
    );
    assert_eq!(
        response.headers().get(header::REFERRER_POLICY),
        Some(&header::HeaderValue::from_static("no-referrer"))
    );
}

#[tokio::test]
async fn invalid_request_id_is_replaced_with_a_uuid() {
    let response = collectionops_backend::app()
        .oneshot(
            Request::builder()
                .uri("/api/v1/health/live")
                .header(collectionops_backend::REQUEST_ID_HEADER, "untrusted-value")
                .body(Body::empty())
                .expect("request must be valid"),
        )
        .await
        .expect("router must answer");

    let request_id = response
        .headers()
        .get(collectionops_backend::REQUEST_ID_HEADER)
        .expect("response must contain a request ID")
        .to_str()
        .expect("request ID must be text");

    Uuid::parse_str(request_id).expect("generated request ID must be a UUID");
}
