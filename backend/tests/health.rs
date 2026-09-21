use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use serde_json::Value;
use tower::ServiceExt;

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

    let body = to_bytes(response.into_body(), 64 * 1024)
        .await
        .expect("response body must be readable");
    let payload: Value = serde_json::from_slice(&body).expect("response must be valid JSON");

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

    let body = to_bytes(response.into_body(), 256 * 1024)
        .await
        .expect("response body must be readable");
    let payload: Value = serde_json::from_slice(&body).expect("response must be valid JSON");

    assert!(payload["paths"]["/api/v1/health"].is_object());
}

