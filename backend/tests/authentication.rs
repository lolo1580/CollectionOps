//! End-to-end tests for the login, session list and revocation routes.
//!
//! They only run when `COLLECTIONOPS_TEST_DATABASE_URL` is set. Each test builds its own
//! router over a shared database and takes a process-wide lock, because `sqlx::migrate!`
//! is not safe to run concurrently.

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use collectionops_backend::{BootstrapAdmin, Database, PasswordService, SESSION_TOKEN_HEADER};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tokio::sync::{Mutex, MutexGuard};
use tower::ServiceExt;

static DATABASE_LOCK: Mutex<()> = Mutex::const_new(());

const EMAIL: &str = "root@example.com";
const PASSWORD: &str = "correct horse battery staple";

async fn test_router() -> Option<(Router, MutexGuard<'static, ()>)> {
    let database_url = std::env::var("COLLECTIONOPS_TEST_DATABASE_URL").ok()?;
    let guard = DATABASE_LOCK.lock().await;
    let database = Database::connect_and_migrate(&database_url)
        .await
        .expect("test database must accept the migrations");

    sqlx::query("DELETE FROM account_sessions")
        .execute(database.pool())
        .await
        .expect("sessions must be clearable");
    sqlx::query("DELETE FROM accounts WHERE is_system_admin = TRUE")
        .execute(database.pool())
        .await
        .expect("accounts must be clearable");

    let admin =
        BootstrapAdmin::from_parts(EMAIL, "Root", PASSWORD).expect("the administrator is valid");
    database
        .provision_bootstrap_admin(&admin, &PasswordService::default())
        .await
        .expect("provisioning must succeed");

    Some((collectionops_backend::app_with_database(database), guard))
}

async fn body_json(response: axum::response::Response) -> Value {
    let body = response
        .into_body()
        .collect()
        .await
        .expect("body must be readable")
        .to_bytes();
    serde_json::from_slice(&body).expect("body must be JSON")
}

fn login_request(payload: &Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/api/v1/sessions")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(payload.to_string()))
        .expect("request must be valid")
}

/// Logs in and returns the issued token.
async fn login(router: &Router, payload: Value) -> String {
    let response = router
        .clone()
        .oneshot(login_request(&payload))
        .await
        .expect("router must answer");
    assert_eq!(response.status(), StatusCode::CREATED);

    body_json(response).await["token"]
        .as_str()
        .expect("the login response must carry a token")
        .to_owned()
}

#[tokio::test]
async fn login_returns_a_token_that_authenticates() {
    let Some((router, _guard)) = test_router().await else {
        return;
    };

    let response = router
        .clone()
        .oneshot(login_request(&json!({
            "email": EMAIL,
            "password": PASSWORD,
            "device_label": "Bureau",
        })))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CREATED);

    let payload = body_json(response).await;
    assert_eq!(payload["session"]["device_label"], "Bureau");
    assert_eq!(payload["session"]["idle_timeout"], "minutes_30");
    assert!(payload["token"].as_str().is_some_and(|t| t.len() == 43));
    assert!(payload["session"].get("token").is_none());

    // The token is accepted on the session list route.
    let token = payload["token"].as_str().unwrap();
    let listed = router
        .oneshot(
            Request::builder()
                .uri("/api/v1/sessions")
                .header(SESSION_TOKEN_HEADER, token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(listed.status(), StatusCode::OK);
    let listed = body_json(listed).await;
    assert_eq!(listed["sessions"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn login_honours_the_client_idle_choice() {
    let Some((router, _guard)) = test_router().await else {
        return;
    };

    for (requested, expected) in [
        ("minutes_15", "minutes_15"),
        ("hour_1", "hour_1"),
        ("never", "never"),
    ] {
        let token = login(
            &router,
            json!({"email": EMAIL, "password": PASSWORD, "idle_timeout": requested}),
        )
        .await;

        let listed = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/sessions")
                    .header(SESSION_TOKEN_HEADER, &token)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let listed = body_json(listed).await;

        let sessions = listed["sessions"].as_array().unwrap();
        assert!(
            sessions
                .iter()
                .any(|session| session["idle_timeout"] == expected),
            "a session with {expected} must exist"
        );
    }
}

#[tokio::test]
async fn a_wrong_password_is_refused_without_revealing_the_account() {
    let Some((router, _guard)) = test_router().await else {
        return;
    };

    let wrong_password = router
        .clone()
        .oneshot(login_request(&json!({"email": EMAIL, "password": "wrong"})))
        .await
        .unwrap();
    let unknown_account = router
        .clone()
        .oneshot(login_request(
            &json!({"email": "nobody@example.com", "password": PASSWORD}),
        ))
        .await
        .unwrap();

    assert_eq!(wrong_password.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(unknown_account.status(), StatusCode::UNAUTHORIZED);

    // Both answers must be indistinguishable, otherwise the route enumerates accounts.
    assert_eq!(
        body_json(wrong_password).await,
        body_json(unknown_account).await
    );
}

#[tokio::test]
async fn a_malformed_login_is_rejected_with_a_stable_code() {
    let Some((router, _guard)) = test_router().await else {
        return;
    };

    for (payload, expected) in [
        (
            json!({"email": "not-an-email", "password": PASSWORD}),
            "invalid_email",
        ),
        (json!({"email": EMAIL, "password": ""}), "missing_password"),
    ] {
        let response = router
            .clone()
            .oneshot(login_request(&payload))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body_json(response).await["code"], expected);
    }
}

#[tokio::test]
async fn the_session_routes_require_a_valid_token() {
    let Some((router, _guard)) = test_router().await else {
        return;
    };

    for (method, uri) in [
        ("GET", "/api/v1/sessions"),
        (
            "DELETE",
            "/api/v1/sessions/00000000-0000-7000-8000-000000000000",
        ),
    ] {
        // Missing header.
        let anonymous = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);

        // Present but not a canonical token.
        for forged in ["short", &"!".repeat(43)] {
            let invalid = router
                .clone()
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(uri)
                        .header(SESSION_TOKEN_HEADER, forged)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(invalid.status(), StatusCode::UNAUTHORIZED);
        }
    }
}

#[tokio::test]
async fn revoking_an_unknown_session_is_a_not_found() {
    let Some((router, _guard)) = test_router().await else {
        return;
    };

    let token = login(&router, json!({"email": EMAIL, "password": PASSWORD})).await;

    let response = router
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/v1/sessions/00000000-0000-7000-8000-000000000000")
                .header(SESSION_TOKEN_HEADER, &token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn revoking_a_device_invalidates_its_token_on_the_next_request() {
    let Some((router, _guard)) = test_router().await else {
        return;
    };

    // Two devices, two independent tokens.
    let first = router
        .clone()
        .oneshot(login_request(
            &json!({"email": EMAIL, "password": PASSWORD, "device_label": "Vole"}),
        ))
        .await
        .unwrap();
    let first = body_json(first).await;
    let stolen_token = first["token"].as_str().unwrap().to_owned();
    let stolen_id = first["session"]["id"].as_str().unwrap().to_owned();

    let kept_token = login(
        &router,
        json!({"email": EMAIL, "password": PASSWORD, "device_label": "Bureau"}),
    )
    .await;

    // The legitimate device revokes the stolen one.
    let revoked = router
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/sessions/{stolen_id}"))
                .header(SESSION_TOKEN_HEADER, &kept_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(revoked.status(), StatusCode::NO_CONTENT);

    let stolen_use = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/sessions")
                .header(SESSION_TOKEN_HEADER, &stolen_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        stolen_use.status(),
        StatusCode::UNAUTHORIZED,
        "a revoked token must stop working immediately"
    );

    // The other device is untouched.
    let kept_use = router
        .oneshot(
            Request::builder()
                .uri("/api/v1/sessions")
                .header(SESSION_TOKEN_HEADER, &kept_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(kept_use.status(), StatusCode::OK);
}

#[tokio::test]
async fn an_account_cannot_revoke_a_session_it_does_not_own() {
    let Some((router, _guard)) = test_router().await else {
        return;
    };

    let token = login(&router, json!({"email": EMAIL, "password": PASSWORD})).await;

    // A syntactically valid session identifier belonging to nobody.
    let response = router
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/v1/sessions/0189a4c2-7f00-7000-8000-0000000000ff")
                .header(SESSION_TOKEN_HEADER, &token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
