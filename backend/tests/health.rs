use axum::{
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
