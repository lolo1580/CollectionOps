use axum::{
    Extension, Json, Router,
    body::Body,
    extract::Request,
    http::{HeaderValue, StatusCode, Uri, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use serde::{Deserialize, Serialize};
use tower_http::trace::TraceLayer;
use utoipa::{OpenApi, ToSchema};
use uuid::Uuid;

use crate::security::Principal;

pub const API_PREFIX: &str = "/api/v1";
pub const REQUEST_ID_HEADER: &str = "x-request-id";

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct HealthResponse {
    pub status: &'static str,
    pub service: &'static str,
    pub version: &'static str,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ServiceInfoResponse {
    pub service: &'static str,
    pub version: &'static str,
    pub api_version: &'static str,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ProblemDetails {
    #[serde(rename = "type")]
    pub type_url: &'static str,
    pub title: &'static str,
    pub status: u16,
    pub detail: String,
    pub instance: String,
    pub code: &'static str,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SessionResponse {
    pub principal: Principal,
}

struct ApiError {
    status: StatusCode,
    problem: ProblemDetails,
}

impl ApiError {
    fn authentication_required(instance: &str) -> Self {
        let status = StatusCode::UNAUTHORIZED;
        Self {
            status,
            problem: ProblemDetails {
                type_url: "about:blank",
                title: "Authentification requise",
                status: status.as_u16(),
                detail: "Une session valide est nécessaire pour accéder à cette ressource."
                    .to_owned(),
                instance: instance.to_owned(),
                code: "authentication_required",
            },
        }
    }

    fn route_not_found(instance: &str) -> Self {
        let status = StatusCode::NOT_FOUND;
        Self {
            status,
            problem: ProblemDetails {
                type_url: "about:blank",
                title: "Ressource introuvable",
                status: status.as_u16(),
                detail: "Aucune route ne correspond à cette requête.".to_owned(),
                instance: instance.to_owned(),
                code: "route_not_found",
            },
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut response = (self.status, Json(self.problem)).into_response();
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/problem+json"),
        );
        response
    }
}

#[utoipa::path(
    get,
    path = "/api/v1",
    tag = "system",
    responses(
        (status = 200, description = "Informations sur le service", body = ServiceInfoResponse)
    )
)]
async fn service_info() -> Json<ServiceInfoResponse> {
    Json(ServiceInfoResponse {
        service: "collectionops-backend",
        version: env!("CARGO_PKG_VERSION"),
        api_version: "v1",
    })
}

#[utoipa::path(
    get,
    path = "/api/v1/health",
    tag = "system",
    responses(
        (status = 200, description = "Le backend est disponible", body = HealthResponse)
    )
)]
async fn health() -> Json<HealthResponse> {
    health_payload("ok")
}

#[utoipa::path(
    get,
    path = "/api/v1/health/live",
    tag = "system",
    responses(
        (status = 200, description = "Le processus est actif", body = HealthResponse)
    )
)]
async fn liveness() -> Json<HealthResponse> {
    health_payload("alive")
}

#[utoipa::path(
    get,
    path = "/api/v1/health/ready",
    tag = "system",
    responses(
        (status = 200, description = "Le service est prêt à recevoir des requêtes", body = HealthResponse)
    )
)]
async fn readiness() -> Json<HealthResponse> {
    // External dependencies will be added here when MariaDB and document storage are introduced.
    health_payload("ready")
}

#[utoipa::path(
    get,
    path = "/api/v1/session",
    tag = "authentication",
    responses(
        (status = 200, description = "Session active et permissions applicatives", body = SessionResponse),
        (status = 401, description = "Authentification requise", body = ProblemDetails)
    )
)]
async fn session(
    uri: Uri,
    principal: Option<Extension<Principal>>,
) -> Result<Json<SessionResponse>, ApiError> {
    principal.map_or_else(
        || Err(ApiError::authentication_required(uri.path())),
        |Extension(principal)| Ok(Json(SessionResponse { principal })),
    )
}

fn health_payload(status: &'static str) -> Json<HealthResponse> {
    Json(HealthResponse {
        status,
        service: "collectionops-backend",
        version: env!("CARGO_PKG_VERSION"),
    })
}

#[derive(OpenApi)]
#[openapi(
    info(
        title = "CollectionOps API",
        version = "0.1.0",
        description = "API contract for CollectionOps clients"
    ),
    paths(service_info, health, liveness, readiness, session),
    components(schemas(
        HealthResponse,
        ServiceInfoResponse,
        ProblemDetails,
        SessionResponse,
        Principal,
        crate::security::Permission
    )),
    tags(
        (name = "system", description = "État et métadonnées du service"),
        (name = "authentication", description = "Session et identité applicative")
    )
)]
pub struct ApiDoc;

async fn openapi() -> Json<utoipa::openapi::OpenApi> {
    Json(ApiDoc::openapi())
}

async fn not_found(uri: Uri) -> ApiError {
    ApiError::route_not_found(uri.path())
}

async fn request_context(mut request: Request, next: Next) -> Response {
    let request_id = request
        .headers()
        .get(REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| Uuid::parse_str(value).ok())
        .unwrap_or_else(Uuid::now_v7)
        .to_string();
    let request_id = HeaderValue::from_str(&request_id).expect("a UUID is always a valid header");

    request
        .headers_mut()
        .insert(REQUEST_ID_HEADER, request_id.clone());

    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(REQUEST_ID_HEADER, request_id);
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );

    response
}

/// Builds the `CollectionOps` HTTP router.
pub fn app() -> Router {
    Router::new()
        .route(API_PREFIX, get(service_info))
        .route(&format!("{API_PREFIX}/health"), get(health))
        .route(&format!("{API_PREFIX}/health/live"), get(liveness))
        .route(&format!("{API_PREFIX}/health/ready"), get(readiness))
        .route(&format!("{API_PREFIX}/session"), get(session))
        .route("/api/openapi.json", get(openapi))
        .fallback(not_found)
        .layer(
            TraceLayer::new_for_http().make_span_with(|request: &Request<Body>| {
                let request_id = request
                    .headers()
                    .get(REQUEST_ID_HEADER)
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or("unknown");

                tracing::info_span!(
                    "http_request",
                    method = %request.method(),
                    request_id
                )
            }),
        )
        .layer(middleware::from_fn(request_context))
}
