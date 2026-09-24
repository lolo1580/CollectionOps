use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use axum::{
    Extension, Json, Router,
    body::Body,
    extract::{Path, Query, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, Uri, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
};
use serde::{Deserialize, Serialize};
use tower_http::trace::TraceLayer;
use utoipa::{OpenApi, ToSchema};
use uuid::Uuid;

use crate::{
    acquisitions::{AcquisitionError, Offer, Vendor, Wish},
    credentials::{
        EmailAddress, IdleTimeout, InvitationToken, PasswordService, SessionToken,
        SessionTokenError,
    },
    database::{
        Database, DatabaseError, InventoryError, IssuedSession, Item, ItemSearchFilters, ItemState,
        ItemStateEvent, ItemTransfer, MemberWithAccount, SessionError, SessionRecord, Space,
        SpaceError, SpaceOwnershipTransfer, TransferOutcome,
    },
    groups::{GroupError, InventoryGroup},
    invitations::{Invitation, InvitationError},
    locations::{ItemLocationEvent, Location, LocationError},
    mail::SmtpDelivery,
    relations::{ItemRelation, MAX_ITEM_RELATIONS, RelationError, RelationKind},
    security::{Permission, Principal},
    taxonomy::{Category, CategoryField, EffectiveField, FieldType, ItemCategory, TaxonomyError},
};

pub const API_PREFIX: &str = "/api/v1";
pub const REQUEST_ID_HEADER: &str = "x-request-id";
/// Header carrying the opaque session token. It is never a cookie and never a query parameter,
/// so a token cannot leak through a `Referer` header or a browser history.
pub const SESSION_TOKEN_HEADER: &str = "x-session-token";

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

/// Identity and application permissions of the current session.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SessionResponse {
    pub principal: Principal,
}

/// A session as shown to its owner. It never contains the token or its fingerprint.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SessionSummary {
    pub id: String,
    pub device_label: Option<String>,
    pub idle_timeout: IdleTimeout,
    pub created_at: String,
    pub last_seen_at: String,
    pub absolute_expires_at: String,
    pub revoked_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SessionListResponse {
    pub sessions: Vec<SessionSummary>,
}

/// Credentials for `POST /api/v1/sessions`.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
    /// Label shown in the device list, e.g. the machine name.
    pub device_label: Option<String>,
    /// Idle delay the client locks itself after. Defaults to 30 minutes.
    pub idle_timeout: Option<IdleTimeout>,
    /// Asks for the longest token lifetime the server allows, still capped at seven days.
    pub stay_signed_in: Option<bool>,
}

/// The only response that ever carries the bearer token.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct LoginResponse {
    pub session: SessionSummary,
    pub token: String,
    pub principal: Principal,
}

impl From<SessionRecord> for SessionSummary {
    fn from(record: SessionRecord) -> Self {
        Self {
            id: record.id,
            device_label: record.device_label,
            idle_timeout: record.idle_timeout,
            created_at: record.created_at.to_rfc3339(),
            last_seen_at: record.last_seen_at.to_rfc3339(),
            absolute_expires_at: record.absolute_expires_at.to_rfc3339(),
            revoked_at: record.revoked_at.map(|at| at.to_rfc3339()),
        }
    }
}

/// Shared state of the router.
///
/// `database` is optional because the service still starts without persistence. The login
/// routes are absent in that case, so a request can never reach a missing pool.
#[derive(Clone)]
struct AppState {
    database: Option<Arc<Database>>,
    passwords: Arc<PasswordService>,
    mail: Option<Arc<SmtpDelivery>>,
}

struct ApiError {
    status: StatusCode,
    problem: ProblemDetails,
}

impl ApiError {
    fn resource_not_found(instance: &str) -> Self {
        let mut error = Self::route_not_found(instance);
        "Ressource introuvable ou inaccessible.".clone_into(&mut error.problem.detail);
        error.problem.code = "resource_not_found";
        error
    }

    fn forbidden(instance: &str) -> Self {
        let status = StatusCode::FORBIDDEN;
        Self {
            status,
            problem: ProblemDetails {
                type_url: "about:blank",
                title: "Accès refusé",
                status: status.as_u16(),
                detail: "La permission requise manque dans cet espace.".to_owned(),
                instance: instance.to_owned(),
                code: "permission_denied",
            },
        }
    }

    fn revision_conflict(instance: &str) -> Self {
        let status = StatusCode::CONFLICT;
        Self {
            status,
            problem: ProblemDetails {
                type_url: "about:blank",
                title: "Conflit de revision",
                status: status.as_u16(),
                detail: "La ressource a change depuis sa derniere lecture. Actualisez-la avant de recommencer."
                    .to_owned(),
                instance: instance.to_owned(),
                code: "revision_conflict",
            },
        }
    }

    fn conflict(instance: &str, detail: &str, code: &'static str) -> Self {
        let status = StatusCode::CONFLICT;
        Self {
            status,
            problem: ProblemDetails {
                type_url: "about:blank",
                title: "Conflit",
                status: status.as_u16(),
                detail: detail.to_owned(),
                instance: instance.to_owned(),
                code,
            },
        }
    }

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

    /// Rejects a login attempt without revealing whether the address exists.
    fn invalid_credentials(instance: &str) -> Self {
        let status = StatusCode::UNAUTHORIZED;
        Self {
            status,
            problem: ProblemDetails {
                type_url: "about:blank",
                title: "Identifiants refusés",
                status: status.as_u16(),
                detail: "L'adresse ou le mot de passe est incorrect.".to_owned(),
                instance: instance.to_owned(),
                code: "invalid_credentials",
            },
        }
    }

    fn invalid_request(instance: &str, detail: impl Into<String>, code: &'static str) -> Self {
        let status = StatusCode::UNPROCESSABLE_ENTITY;
        Self {
            status,
            problem: ProblemDetails {
                type_url: "about:blank",
                title: "Requête invalide",
                status: status.as_u16(),
                detail: detail.into(),
                instance: instance.to_owned(),
                code,
            },
        }
    }

    fn session_not_found(instance: &str) -> Self {
        let status = StatusCode::NOT_FOUND;
        Self {
            status,
            problem: ProblemDetails {
                type_url: "about:blank",
                title: "Session introuvable",
                status: status.as_u16(),
                detail: "Aucune session active ne correspond à cet identifiant.".to_owned(),
                instance: instance.to_owned(),
                code: "session_not_found",
            },
        }
    }

    /// Persistence is unavailable, so the operation cannot be attempted at all.
    fn service_unavailable(instance: &str) -> Self {
        let status = StatusCode::SERVICE_UNAVAILABLE;
        Self {
            status,
            problem: ProblemDetails {
                type_url: "about:blank",
                title: "Service indisponible",
                status: status.as_u16(),
                detail: "La persistance n'est pas configurée sur ce déploiement.".to_owned(),
                instance: instance.to_owned(),
                code: "persistence_unavailable",
            },
        }
    }

    fn mail_unavailable(instance: &str) -> Self {
        let status = StatusCode::SERVICE_UNAVAILABLE;
        Self {
            status,
            problem: ProblemDetails {
                type_url: "about:blank",
                title: "Envoi indisponible",
                status: status.as_u16(),
                detail: "L'envoi des invitations n'est pas configuré sur ce serveur.".to_owned(),
                instance: instance.to_owned(),
                code: "mail_unavailable",
            },
        }
    }

    fn internal(instance: &str) -> Self {
        let status = StatusCode::INTERNAL_SERVER_ERROR;
        Self {
            status,
            problem: ProblemDetails {
                type_url: "about:blank",
                title: "Erreur interne",
                status: status.as_u16(),
                detail: "La requête n'a pas pu être traitée.".to_owned(),
                instance: instance.to_owned(),
                code: "internal_error",
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
        (status = 200, description = "Le service et sa base configurée sont prêts", body = HealthResponse),
        (status = 503, description = "La base configurée ne répond pas", body = HealthResponse)
    )
)]
async fn readiness(State(state): State<AppState>) -> (StatusCode, Json<HealthResponse>) {
    if let Some(database) = state.database.as_deref() {
        let check = sqlx::query("SELECT 1").execute(database.pool());
        if !matches!(
            tokio::time::timeout(Duration::from_secs(2), check).await,
            Ok(Ok(_))
        ) {
            return (StatusCode::SERVICE_UNAVAILABLE, health_payload("not_ready"));
        }
    }
    (StatusCode::OK, health_payload("ready"))
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

/// Extracts the opaque session token from its dedicated header.
///
/// The token is deliberately not read from a cookie or a query parameter: a `Referer` header,
/// a browser history entry, or a proxy log would otherwise capture it.
fn session_token(request: &Request) -> Result<SessionToken, ApiError> {
    let instance = request.uri().path().to_owned();

    request
        .headers()
        .get(SESSION_TOKEN_HEADER)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| ApiError::authentication_required(&instance))
        .and_then(|value| {
            SessionToken::parse(value)
                .map_err(|_: SessionTokenError| ApiError::authentication_required(&instance))
        })
}

#[utoipa::path(
    post,
    path = "/api/v1/sessions",
    tag = "authentication",
    request_body = LoginRequest,
    responses(
        (status = 201, description = "Session créée et jeton émis", body = LoginResponse),
        (status = 401, description = "Identifiants refusés", body = ProblemDetails),
        (status = 422, description = "Requête invalide", body = ProblemDetails),
        (status = 503, description = "Persistance non configurée", body = ProblemDetails)
    )
)]
async fn create_session(
    State(state): State<AppState>,
    uri: Uri,
    Json(payload): Json<LoginRequest>,
) -> Result<(StatusCode, Json<LoginResponse>), ApiError> {
    let instance = uri.path().to_owned();
    let Some(database) = state.database.as_deref() else {
        return Err(ApiError::service_unavailable(&instance));
    };

    let email = EmailAddress::parse(&payload.email).map_err(|_| {
        ApiError::invalid_request(&instance, "L'adresse e-mail est invalide.", "invalid_email")
    })?;

    if payload.password.is_empty() {
        return Err(ApiError::invalid_request(
            &instance,
            "Le mot de passe est obligatoire.",
            "missing_password",
        ));
    }

    let issued = database
        .create_session(
            &email,
            &payload.password,
            payload.device_label.as_deref(),
            payload.idle_timeout.unwrap_or_default(),
            payload.stay_signed_in.unwrap_or(false),
            &state.passwords,
        )
        .await
        .map_err(|error| match error {
            // A wrong password and an unknown address are reported identically, so the
            // endpoint cannot be used to discover which accounts exist.
            DatabaseError::InvalidCredentials => ApiError::invalid_credentials(&instance),
            _ => ApiError::internal(&instance),
        })?;

    Ok((StatusCode::CREATED, Json(login_response(issued))))
}

#[utoipa::path(
    get,
    path = "/api/v1/sessions",
    tag = "authentication",
    responses(
        (status = 200, description = "Sessions de l'appareil courant et des autres appareils", body = SessionListResponse),
        (status = 401, description = "Authentification requise", body = ProblemDetails),
        (status = 503, description = "Persistance non configurée", body = ProblemDetails)
    )
)]
async fn list_sessions(
    State(state): State<AppState>,
    uri: Uri,
    request: Request,
) -> Result<Json<SessionListResponse>, ApiError> {
    let instance = uri.path().to_owned();
    let token = session_token(&request)?;
    let Some(database) = state.database.as_deref() else {
        return Err(ApiError::service_unavailable(&instance));
    };

    let authenticated = database
        .authenticate_session(&token)
        .await
        .map_err(|error| session_error_to_api(&instance, &error))?;

    let sessions = database
        .list_sessions(&authenticated.account_id)
        .await
        .map_err(|_| ApiError::internal(&instance))?;

    Ok(Json(SessionListResponse {
        sessions: sessions.into_iter().map(SessionSummary::from).collect(),
    }))
}

#[utoipa::path(
    delete,
    path = "/api/v1/sessions/{session_id}",
    tag = "authentication",
    params(("session_id" = String, Path, description = "Identifiant de la session à révoquer")),
    responses(
        (status = 204, description = "Session révoquée"),
        (status = 401, description = "Authentification requise", body = ProblemDetails),
        (status = 404, description = "Session introuvable", body = ProblemDetails),
        (status = 503, description = "Persistance non configurée", body = ProblemDetails)
    )
)]
async fn revoke_session(
    State(state): State<AppState>,
    uri: Uri,
    Path(session_id): Path<String>,
    request: Request,
) -> Result<StatusCode, ApiError> {
    let instance = uri.path().to_owned();
    let token = session_token(&request)?;
    let Some(database) = state.database.as_deref() else {
        return Err(ApiError::service_unavailable(&instance));
    };

    let authenticated = database
        .authenticate_session(&token)
        .await
        .map_err(|error| session_error_to_api(&instance, &error))?;

    let sessions = database
        .list_sessions(&authenticated.account_id)
        .await
        .map_err(|_| ApiError::internal(&instance))?;

    // Only the owner may revoke, and only a session that belongs to that account.
    if !sessions.iter().any(|session| session.id == session_id) {
        return Err(ApiError::session_not_found(&instance));
    }

    if !database
        .revoke_session(&session_id)
        .await
        .map_err(|_| ApiError::internal(&instance))?
    {
        return Err(ApiError::session_not_found(&instance));
    }

    Ok(StatusCode::NO_CONTENT)
}

fn login_response(issued: IssuedSession) -> LoginResponse {
    // Read the token before moving the session out of `issued`.
    let token = issued.token().expose_secret().to_owned();

    LoginResponse {
        session: SessionSummary::from(issued.session),
        token,
        principal: collection_principal(
            issued.account_id,
            issued.display_name,
            issued.is_system_admin,
        ),
    }
}

// Application-level collection and acquisition capabilities still require an explicit space grant.
fn collection_principal(subject: Uuid, display_name: String, is_system_admin: bool) -> Principal {
    Principal {
        subject,
        display_name,
        roles: if is_system_admin {
            ["system_admin".to_owned()].into()
        } else {
            BTreeSet::default()
        },
        permissions: [
            Permission::CollectionsRead,
            Permission::CollectionsWrite,
            Permission::AcquisitionsRead,
            Permission::AcquisitionsWrite,
        ]
        .into(),
    }
}

async fn authenticated_headers(
    database: &Database,
    headers: &HeaderMap,
    instance: &str,
) -> Result<Principal, ApiError> {
    let value = headers
        .get(SESSION_TOKEN_HEADER)
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| ApiError::authentication_required(instance))?;
    let token =
        SessionToken::parse(value).map_err(|_| ApiError::authentication_required(instance))?;
    let session = database
        .authenticate_session(&token)
        .await
        .map_err(|error| session_error_to_api(instance, &error))?;
    let subject = Uuid::parse_str(&session.account_id).map_err(|_| ApiError::internal(instance))?;
    Ok(collection_principal(
        subject,
        session.display_name,
        session.is_system_admin,
    ))
}

async fn authorized_space(
    database: &Database,
    principal: &Principal,
    space_id: Uuid,
    permission: Permission,
    instance: &str,
) -> Result<Space, ApiError> {
    let membership = database
        .membership(space_id, principal.subject)
        .await
        .map_err(|_| ApiError::internal(instance))?
        .ok_or_else(|| ApiError::resource_not_found(instance))?;
    principal
        .require_space_permission(
            space_id,
            Some(&membership.as_space_membership()),
            permission,
        )
        .map_err(|_| ApiError::forbidden(instance))?;
    database
        .space(space_id)
        .await
        .map_err(|error| match error.space_error() {
            Some(SpaceError::SpaceNotFound) => ApiError::resource_not_found(instance),
            _ => ApiError::internal(instance),
        })
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct SpaceResponse {
    pub id: Uuid,
    pub name: String,
    pub owner_account_id: Uuid,
    pub created_at: String,
}
impl From<Space> for SpaceResponse {
    fn from(value: Space) -> Self {
        Self {
            id: value.id,
            name: value.name,
            owner_account_id: value.owner_account_id,
            created_at: value.created_at.to_rfc3339(),
        }
    }
}
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct SpaceListResponse {
    pub spaces: Vec<SpaceResponse>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct SpacePermissionsResponse {
    pub permissions: Vec<Permission>,
}

fn acquisition_error(error: &AcquisitionError, instance: &str) -> ApiError {
    match error {
        AcquisitionError::InvalidTitle => {
            ApiError::invalid_request(instance, "Nom invalide.", "invalid_title")
        }
        AcquisitionError::InvalidNotes => {
            ApiError::invalid_request(instance, "Notes trop longues.", "invalid_notes")
        }
        AcquisitionError::InvalidUrl => {
            ApiError::invalid_request(instance, "Adresse web invalide.", "invalid_url")
        }
        AcquisitionError::DuplicateVendor => {
            ApiError::conflict(instance, "Ce fournisseur existe déjà.", "duplicate_vendor")
        }
        AcquisitionError::WishNotFound
        | AcquisitionError::VendorNotFound
        | AcquisitionError::OfferNotFound => ApiError::resource_not_found(instance),
        AcquisitionError::RevisionConflict => ApiError::revision_conflict(instance),
        _ => ApiError::internal(instance),
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct WishResponse {
    pub id: Uuid,
    pub space_id: Uuid,
    pub title: String,
    pub search_notes: Option<String>,
    pub revision: String,
    pub created_at: String,
}
impl From<Wish> for WishResponse {
    fn from(value: Wish) -> Self {
        Self {
            id: value.id,
            space_id: value.space_id,
            title: value.title,
            search_notes: value.search_notes,
            revision: value.revision.to_string(),
            created_at: value.created_at.to_rfc3339(),
        }
    }
}
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct WishListResponse {
    pub wishes: Vec<WishResponse>,
}
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CreateWishRequest {
    pub title: String,
    pub search_notes: Option<String>,
}
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct UpdateWishRequest {
    pub title: String,
    pub search_notes: Option<String>,
    pub expected_revision: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct VendorResponse {
    pub id: Uuid,
    pub space_id: Uuid,
    pub name: String,
    pub website_url: Option<String>,
    pub revision: String,
    pub created_at: String,
}
impl From<Vendor> for VendorResponse {
    fn from(value: Vendor) -> Self {
        Self {
            id: value.id,
            space_id: value.space_id,
            name: value.name,
            website_url: value.website_url,
            revision: value.revision.to_string(),
            created_at: value.created_at.to_rfc3339(),
        }
    }
}
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct VendorListResponse {
    pub vendors: Vec<VendorResponse>,
}
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CreateVendorRequest {
    pub name: String,
    pub website_url: Option<String>,
}
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct UpdateVendorRequest {
    pub name: String,
    pub website_url: Option<String>,
    pub expected_revision: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct OfferResponse {
    pub id: Uuid,
    pub space_id: Uuid,
    pub wish_id: Uuid,
    pub vendor_id: Uuid,
    pub title: String,
    pub source_url: Option<String>,
    pub notes: Option<String>,
    pub revision: String,
    pub created_at: String,
}
impl From<Offer> for OfferResponse {
    fn from(value: Offer) -> Self {
        Self {
            id: value.id,
            space_id: value.space_id,
            wish_id: value.wish_id,
            vendor_id: value.vendor_id,
            title: value.title,
            source_url: value.source_url,
            notes: value.notes,
            revision: value.revision.to_string(),
            created_at: value.created_at.to_rfc3339(),
        }
    }
}
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct OfferListResponse {
    pub offers: Vec<OfferResponse>,
}
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CreateOfferRequest {
    pub vendor_id: Uuid,
    pub title: String,
    pub source_url: Option<String>,
    pub notes: Option<String>,
}
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct UpdateOfferRequest {
    pub vendor_id: Uuid,
    pub title: String,
    pub source_url: Option<String>,
    pub notes: Option<String>,
    pub expected_revision: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CategoryResponse {
    pub id: Uuid,
    pub space_id: Uuid,
    pub parent_id: Option<Uuid>,
    pub name: String,
    pub created_at: String,
}

impl From<Category> for CategoryResponse {
    fn from(value: Category) -> Self {
        Self {
            id: value.id,
            space_id: value.space_id,
            parent_id: value.parent_id,
            name: value.name,
            created_at: value.created_at.to_rfc3339(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CategoryListResponse {
    pub categories: Vec<CategoryResponse>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CreateCategoryRequest {
    pub name: String,
    pub parent_id: Option<Uuid>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CategoryFieldResponse {
    pub id: Uuid,
    pub category_id: Uuid,
    pub name: String,
    pub value_type: String,
    pub created_at: String,
}

impl From<CategoryField> for CategoryFieldResponse {
    fn from(value: CategoryField) -> Self {
        Self {
            id: value.id,
            category_id: value.category_id,
            name: value.name,
            value_type: value.value_type.as_str().to_owned(),
            created_at: value.created_at.to_rfc3339(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CategoryFieldListResponse {
    pub fields: Vec<CategoryFieldResponse>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CreateCategoryFieldRequest {
    pub name: String,
    pub value_type: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct RenameDefinitionRequest {
    pub name: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ItemRelationResponse {
    pub id: Uuid,
    pub kind: String,
    pub direction: String,
    pub related_item_id: Uuid,
    pub related_item_name: String,
    pub created_at: String,
}

impl ItemRelationResponse {
    fn from_relation(value: ItemRelation, item_id: Uuid) -> Self {
        let outgoing = value.source_item_id == item_id;
        Self {
            id: value.id,
            kind: value.kind.as_str().to_owned(),
            direction: if outgoing { "outgoing" } else { "incoming" }.to_owned(),
            related_item_id: if outgoing {
                value.target_item_id
            } else {
                value.source_item_id
            },
            related_item_name: if outgoing {
                value.target_name
            } else {
                value.source_name
            },
            created_at: value.created_at.to_rfc3339(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ItemRelationListResponse {
    pub revision: String,
    pub relations: Vec<ItemRelationResponse>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ItemRelationInput {
    pub target_id: Uuid,
    pub kind: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ReplaceItemRelationsRequest {
    pub relations: Vec<ItemRelationInput>,
    pub expected_revision: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct LocationResponse {
    pub id: Uuid,
    pub space_id: Uuid,
    pub parent_id: Option<Uuid>,
    pub name: String,
    pub created_at: String,
}

impl From<Location> for LocationResponse {
    fn from(value: Location) -> Self {
        Self {
            id: value.id,
            space_id: value.space_id,
            parent_id: value.parent_id,
            name: value.name,
            created_at: value.created_at.to_rfc3339(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct LocationListResponse {
    pub locations: Vec<LocationResponse>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CreateLocationRequest {
    pub name: String,
    pub parent_id: Option<Uuid>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ItemLocationResponse {
    pub revision: String,
    pub location: Option<LocationResponse>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct MoveItemLocationRequest {
    pub location_id: Option<Uuid>,
    pub expected_revision: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ItemLocationEventResponse {
    pub id: Uuid,
    pub from_location_id: Option<Uuid>,
    pub from_location_name: Option<String>,
    pub to_location_id: Option<Uuid>,
    pub to_location_name: Option<String>,
    pub actor_display_name: String,
    pub created_at: String,
}

impl From<ItemLocationEvent> for ItemLocationEventResponse {
    fn from(value: ItemLocationEvent) -> Self {
        Self {
            id: value.id,
            from_location_id: value.from_location_id,
            from_location_name: value.from_location_name,
            to_location_id: value.to_location_id,
            to_location_name: value.to_location_name,
            actor_display_name: value.actor_display_name,
            created_at: value.created_at.to_rfc3339(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ItemLocationEventListResponse {
    pub events: Vec<ItemLocationEventResponse>,
}

fn location_error(error: &LocationError, instance: &str) -> ApiError {
    match error {
        LocationError::InvalidName => ApiError::invalid_request(
            instance,
            "Nom d'emplacement invalide.",
            "invalid_location_name",
        ),
        LocationError::SpaceNotFound
        | LocationError::ParentNotFound
        | LocationError::LocationNotFound
        | LocationError::ItemNotFound => ApiError::resource_not_found(instance),
        LocationError::DuplicateName => ApiError::conflict(
            instance,
            "Cet emplacement existe deja ici.",
            "duplicate_name",
        ),
        LocationError::RevisionConflict => ApiError::revision_conflict(instance),
        LocationError::ItemInTrash => ApiError::invalid_request(
            instance,
            "Restaurez l'objet avant de changer son emplacement.",
            "invalid_state",
        ),
        _ => ApiError::internal(instance),
    }
}

fn group_error(error: &GroupError, instance: &str) -> ApiError {
    match error {
        GroupError::InvalidName | GroupError::InvalidKind => ApiError::invalid_request(
            instance,
            "Nom ou type de regroupement invalide.",
            "invalid_group",
        ),
        GroupError::DuplicateName => {
            ApiError::conflict(instance, "Ce nom existe deja.", "duplicate_name")
        }
        GroupError::GroupNotEmpty => ApiError::conflict(
            instance,
            "Retirez les objets de ce regroupement avant de le supprimer.",
            "group_not_empty",
        ),
        GroupError::SpaceNotFound
        | GroupError::ItemNotFound
        | GroupError::GroupNotFound
        | GroupError::InvalidGroup => ApiError::resource_not_found(instance),
        GroupError::RevisionConflict => ApiError::revision_conflict(instance),
        GroupError::ItemInTrash => ApiError::invalid_request(
            instance,
            "Restaurez l'objet avant de le classer.",
            "invalid_state",
        ),
        _ => ApiError::internal(instance),
    }
}

fn relation_error(error: &RelationError, instance: &str) -> ApiError {
    match error {
        RelationError::InvalidKind => ApiError::invalid_request(
            instance,
            "Type de relation invalide.",
            "invalid_relation_kind",
        ),
        RelationError::SelfRelation => ApiError::invalid_request(
            instance,
            "Un objet ne peut pas etre lie a lui-meme.",
            "self_relation",
        ),
        RelationError::DuplicateRelation => ApiError::invalid_request(
            instance,
            "Une meme relation est presente deux fois.",
            "duplicate_relation",
        ),
        RelationError::TooManyRelations => ApiError::invalid_request(
            instance,
            "Trop de relations sur cet objet.",
            "too_many_relations",
        ),
        RelationError::ItemNotFound | RelationError::InvalidTarget => {
            ApiError::resource_not_found(instance)
        }
        RelationError::RevisionConflict => ApiError::revision_conflict(instance),
        RelationError::ItemInTrash => ApiError::invalid_request(
            instance,
            "Restaurez l'objet avant de modifier ses relations.",
            "invalid_state",
        ),
        _ => ApiError::internal(instance),
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct GroupResponse {
    pub id: Uuid,
    pub space_id: Uuid,
    pub kind: String,
    pub name: String,
    pub revision: String,
    pub created_at: String,
}
impl From<InventoryGroup> for GroupResponse {
    fn from(value: InventoryGroup) -> Self {
        Self {
            id: value.id,
            space_id: value.space_id,
            kind: value.kind,
            name: value.name,
            revision: value.revision.to_string(),
            created_at: value.created_at.to_rfc3339(),
        }
    }
}
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct GroupListResponse {
    pub groups: Vec<GroupResponse>,
}
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CreateGroupRequest {
    pub kind: String,
    pub name: String,
}
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct RenameGroupRequest {
    pub name: String,
    pub expected_revision: String,
}
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct DeleteGroupRequest {
    pub expected_revision: String,
}
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ItemGroupListResponse {
    pub revision: String,
    pub groups: Vec<GroupResponse>,
}
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ReplaceItemGroupsRequest {
    pub group_ids: Vec<Uuid>,
    pub expected_revision: String,
}

fn taxonomy_error(error: &TaxonomyError, instance: &str) -> ApiError {
    match error {
        TaxonomyError::InvalidName | TaxonomyError::InvalidFieldType => ApiError::invalid_request(
            instance,
            "Nom ou type de champ invalide.",
            "invalid_taxonomy",
        ),
        TaxonomyError::SpaceNotFound
        | TaxonomyError::ParentNotFound
        | TaxonomyError::CategoryNotFound
        | TaxonomyError::FieldNotFound
        | TaxonomyError::ItemNotFound
        | TaxonomyError::InvalidCategory
        | TaxonomyError::FieldNotAvailable => ApiError::resource_not_found(instance),
        TaxonomyError::DuplicateName => {
            ApiError::conflict(instance, "Ce nom existe deja ici.", "duplicate_name")
        }
        TaxonomyError::RevisionConflict => ApiError::revision_conflict(instance),
        TaxonomyError::ItemInTrash => ApiError::invalid_request(
            instance,
            "Restaurez l'objet avant de modifier sa fiche.",
            "invalid_state",
        ),
        TaxonomyError::EmptySelection | TaxonomyError::TooManyCategories => {
            ApiError::invalid_request(
                instance,
                "Choisissez entre 1 et 20 categories.",
                "invalid_categories",
            )
        }
        TaxonomyError::InvalidValue => ApiError::invalid_request(
            instance,
            "Valeur incompatible avec le type du champ.",
            "invalid_field_value",
        ),
        _ => ApiError::internal(instance),
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ItemCategoryResponse {
    pub assignment_id: Uuid,
    pub category_id: Uuid,
    pub category_name: String,
    pub assigned_at: String,
}

impl From<ItemCategory> for ItemCategoryResponse {
    fn from(value: ItemCategory) -> Self {
        Self {
            assignment_id: value.assignment_id,
            category_id: value.category_id,
            category_name: value.category_name,
            assigned_at: value.assigned_at.to_rfc3339(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ItemCategoryListResponse {
    pub revision: String,
    pub categories: Vec<ItemCategoryResponse>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct AddItemCategoriesRequest {
    pub category_ids: Vec<Uuid>,
    pub expected_revision: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct EffectiveFieldResponse {
    pub id: Uuid,
    pub defined_category_id: Uuid,
    pub defined_category_name: String,
    pub name: String,
    pub value_type: String,
    pub value: Option<String>,
}

impl From<EffectiveField> for EffectiveFieldResponse {
    fn from(value: EffectiveField) -> Self {
        Self {
            id: value.id,
            defined_category_id: value.defined_category_id,
            defined_category_name: value.defined_category_name,
            name: value.name,
            value_type: value.value_type.as_str().to_owned(),
            value: value.value,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct EffectiveFieldListResponse {
    pub revision: String,
    pub fields: Vec<EffectiveFieldResponse>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct SetItemFieldValueRequest {
    pub value: String,
    pub expected_revision: String,
}
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CreateSpaceRequest {
    pub name: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct SpaceMemberResponse {
    pub account_id: Uuid,
    pub display_name: String,
    pub email: Option<String>,
    pub permissions: Vec<Permission>,
}

impl From<MemberWithAccount> for SpaceMemberResponse {
    fn from(value: MemberWithAccount) -> Self {
        Self {
            account_id: value.membership.account_id,
            display_name: value.display_name,
            email: value.email,
            permissions: value.membership.permissions.into_iter().collect(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct SpaceMemberListResponse {
    pub members: Vec<SpaceMemberResponse>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct UpdateMemberPermissionsRequest {
    pub permissions: Vec<Permission>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct TransferSpaceOwnershipRequest {
    pub new_owner_account_id: Uuid,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct SpaceOwnershipResponse {
    pub space_id: Uuid,
    pub previous_owner_account_id: Uuid,
    pub new_owner_account_id: Uuid,
    pub actor_account_id: Uuid,
    pub transferred_at: String,
}

impl From<SpaceOwnershipTransfer> for SpaceOwnershipResponse {
    fn from(value: SpaceOwnershipTransfer) -> Self {
        Self {
            space_id: value.space_id,
            previous_owner_account_id: value.previous_owner_account_id,
            new_owner_account_id: value.new_owner_account_id,
            actor_account_id: value.actor_account_id,
            transferred_at: value.transferred_at.to_rfc3339(),
        }
    }
}

fn can_manage_members(principal: &Principal, owner_account_id: Uuid) -> bool {
    principal.subject == owner_account_id || principal.roles.contains("system_admin")
}

fn member_error(error: &DatabaseError, instance: &str) -> ApiError {
    match error.space_error() {
        Some(SpaceError::NotMember | SpaceError::SpaceNotFound | SpaceError::NotAuthorized) => {
            ApiError::resource_not_found(instance)
        }
        Some(SpaceError::CannotChangeOwnGrants | SpaceError::OwnerCannotBeRemoved) => {
            ApiError::forbidden(instance)
        }
        Some(SpaceError::OwnerCoreGrantsRequired) => ApiError::invalid_request(
            instance,
            "Le propriétaire doit conserver la lecture et l'écriture de collection.",
            "owner_core_grants_required",
        ),
        Some(SpaceError::PermissionNotSpaceScoped) => ApiError::invalid_request(
            instance,
            "Un droit demandé n'appartient pas à un espace.",
            "invalid_grants",
        ),
        _ => ApiError::internal(instance),
    }
}

fn ownership_error(error: &DatabaseError, instance: &str) -> ApiError {
    match error.space_error() {
        Some(SpaceError::SpaceNotFound | SpaceError::NotAuthorized) => {
            ApiError::resource_not_found(instance)
        }
        Some(SpaceError::OwnershipUnchanged) => ApiError::invalid_request(
            instance,
            "Ce membre est déjà propriétaire de l'espace.",
            "ownership_unchanged",
        ),
        Some(SpaceError::TargetNotMember) => ApiError::invalid_request(
            instance,
            "Le nouveau propriétaire doit déjà être membre de l'espace.",
            "target_not_member",
        ),
        _ => ApiError::internal(instance),
    }
}

#[utoipa::path(get, path = "/api/v1/spaces/{space_id}/members", tag = "sharing",
    responses((status = 200, body = SpaceMemberListResponse), (status = 401, body = ProblemDetails), (status = 404, body = ProblemDetails)))]
async fn list_members(
    State(state): State<AppState>,
    uri: Uri,
    Path(space_id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<SpaceMemberListResponse>, ApiError> {
    let db = state
        .database
        .as_deref()
        .ok_or_else(|| ApiError::service_unavailable(uri.path()))?;
    let principal = authenticated_headers(db, &headers, uri.path()).await?;
    let space = db
        .space(space_id)
        .await
        .map_err(|_| ApiError::resource_not_found(uri.path()))?;
    if !can_manage_members(&principal, space.owner_account_id) {
        return Err(ApiError::resource_not_found(uri.path()));
    }
    let members = db
        .space_members_with_accounts(space_id)
        .await
        .map_err(|_| ApiError::internal(uri.path()))?;
    Ok(Json(SpaceMemberListResponse {
        members: members.into_iter().map(Into::into).collect(),
    }))
}

#[utoipa::path(put, path = "/api/v1/spaces/{space_id}/members/{account_id}/permissions", tag = "sharing",
    request_body = UpdateMemberPermissionsRequest,
    responses((status = 200, body = SpaceMemberResponse), (status = 401, body = ProblemDetails), (status = 404, body = ProblemDetails), (status = 422, body = ProblemDetails)))]
async fn update_member_permissions(
    State(state): State<AppState>,
    uri: Uri,
    Path((space_id, account_id)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
    Json(payload): Json<UpdateMemberPermissionsRequest>,
) -> Result<Json<SpaceMemberResponse>, ApiError> {
    let db = state
        .database
        .as_deref()
        .ok_or_else(|| ApiError::service_unavailable(uri.path()))?;
    let principal = authenticated_headers(db, &headers, uri.path()).await?;
    let space = db
        .space(space_id)
        .await
        .map_err(|_| ApiError::resource_not_found(uri.path()))?;
    if !can_manage_members(&principal, space.owner_account_id) {
        return Err(ApiError::resource_not_found(uri.path()));
    }
    db.set_member_permissions(
        space_id,
        account_id,
        &payload.permissions,
        principal.subject,
    )
    .await
    .map_err(|e| member_error(&e, uri.path()))?;
    let member = db
        .space_members_with_accounts(space_id)
        .await
        .map_err(|_| ApiError::internal(uri.path()))?
        .into_iter()
        .find(|member| member.membership.account_id == account_id)
        .ok_or_else(|| ApiError::internal(uri.path()))?;
    Ok(Json(member.into()))
}

#[utoipa::path(delete, path = "/api/v1/spaces/{space_id}/members/{account_id}", tag = "sharing",
    responses((status = 204), (status = 401, body = ProblemDetails), (status = 404, body = ProblemDetails)))]
async fn remove_member(
    State(state): State<AppState>,
    uri: Uri,
    Path((space_id, account_id)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let db = state
        .database
        .as_deref()
        .ok_or_else(|| ApiError::service_unavailable(uri.path()))?;
    let principal = authenticated_headers(db, &headers, uri.path()).await?;
    let space = db
        .space(space_id)
        .await
        .map_err(|_| ApiError::resource_not_found(uri.path()))?;
    if !can_manage_members(&principal, space.owner_account_id) {
        return Err(ApiError::resource_not_found(uri.path()));
    }
    db.remove_member(space_id, account_id, principal.subject)
        .await
        .map_err(|e| member_error(&e, uri.path()))?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(post, path = "/api/v1/spaces/{space_id}/ownership", tag = "sharing",
    request_body = TransferSpaceOwnershipRequest,
    params(("space_id" = Uuid, Path, description = "Espace")),
    responses((status = 200, body = SpaceOwnershipResponse), (status = 401, body = ProblemDetails),
        (status = 404, body = ProblemDetails), (status = 422, body = ProblemDetails)))]
async fn transfer_space_ownership(
    State(state): State<AppState>,
    uri: Uri,
    Path(space_id): Path<Uuid>,
    headers: HeaderMap,
    Json(payload): Json<TransferSpaceOwnershipRequest>,
) -> Result<Json<SpaceOwnershipResponse>, ApiError> {
    let db = state
        .database
        .as_deref()
        .ok_or_else(|| ApiError::service_unavailable(uri.path()))?;
    let principal = authenticated_headers(db, &headers, uri.path()).await?;
    let space = db
        .space(space_id)
        .await
        .map_err(|_| ApiError::resource_not_found(uri.path()))?;
    // Only the current owner may transfer, and an unrelated account must not learn the space
    // exists: both cases answer 404.
    if space.owner_account_id != principal.subject {
        return Err(ApiError::resource_not_found(uri.path()));
    }
    let transfer = db
        .transfer_space_ownership(space_id, payload.new_owner_account_id, principal.subject)
        .await
        .map_err(|error| ownership_error(&error, uri.path()))?;
    Ok(Json(transfer.into()))
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CreateInvitationRequest {
    pub email: String,
    /// Defaults to collection read when omitted. Empty and non-space grants are rejected.
    pub permissions: Option<Vec<Permission>>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct InvitationResponse {
    pub id: Uuid,
    pub space_id: Uuid,
    pub recipient_email: String,
    pub created_at: String,
    pub expires_at: String,
    pub accepted_at: Option<String>,
    pub revoked_at: Option<String>,
    pub permissions: Vec<Permission>,
}

impl From<Invitation> for InvitationResponse {
    fn from(value: Invitation) -> Self {
        Self {
            id: value.id,
            space_id: value.space_id,
            recipient_email: value.recipient_email,
            created_at: value.created_at.to_rfc3339(),
            expires_at: value.expires_at.to_rfc3339(),
            accepted_at: value.accepted_at.map(|d| d.to_rfc3339()),
            revoked_at: value.revoked_at.map(|d| d.to_rfc3339()),
            permissions: value.permissions.into_iter().collect(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct InvitationListResponse {
    pub invitations: Vec<InvitationResponse>,
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct AcceptInvitationRequest {
    pub token: String,
    pub display_name: Option<String>,
    pub password: Option<String>,
}

fn invitation_error(error: &InvitationError, instance: &str) -> ApiError {
    match error {
        InvitationError::NotFound | InvitationError::NotOwner => {
            ApiError::resource_not_found(instance)
        }
        InvitationError::AlreadyMember => ApiError::invalid_request(
            instance,
            "Cette personne est déjà membre de l'espace.",
            "already_member",
        ),
        InvitationError::ExistingAccountRequiresLogin => {
            ApiError::authentication_required(instance)
        }
        InvitationError::AccountMismatch => ApiError::forbidden(instance),
        InvitationError::InvalidAccount => {
            ApiError::invalid_request(instance, "Nom ou mot de passe invalide.", "invalid_account")
        }
        InvitationError::InvalidGrants => ApiError::invalid_request(
            instance,
            "Choisissez au moins un droit d'espace valide.",
            "invalid_grants",
        ),
        InvitationError::Storage(_) | InvitationError::TokenGeneration => {
            ApiError::internal(instance)
        }
    }
}

#[utoipa::path(post, path = "/api/v1/spaces/{space_id}/invitations", tag = "sharing",
    request_body = CreateInvitationRequest,
    responses((status = 201, body = InvitationResponse), (status = 401, body = ProblemDetails), (status = 404, body = ProblemDetails), (status = 503, body = ProblemDetails)))]
async fn create_invitation(
    State(state): State<AppState>,
    uri: Uri,
    Path(space_id): Path<Uuid>,
    headers: HeaderMap,
    Json(payload): Json<CreateInvitationRequest>,
) -> Result<(StatusCode, Json<InvitationResponse>), ApiError> {
    let db = state
        .database
        .as_deref()
        .ok_or_else(|| ApiError::service_unavailable(uri.path()))?;
    let principal = authenticated_headers(db, &headers, uri.path()).await?;
    let mail = state
        .mail
        .as_deref()
        .ok_or_else(|| ApiError::mail_unavailable(uri.path()))?;
    let email = EmailAddress::parse(&payload.email).map_err(|_| {
        ApiError::invalid_request(uri.path(), "Adresse e-mail invalide.", "invalid_email")
    })?;
    let space = db
        .space(space_id)
        .await
        .map_err(|_| ApiError::resource_not_found(uri.path()))?;
    if space.owner_account_id != principal.subject {
        return Err(ApiError::resource_not_found(uri.path()));
    }
    let permissions = payload
        .permissions
        .unwrap_or_else(|| vec![Permission::CollectionsRead]);
    let issued = db
        .issue_invitation_with_grants(space_id, principal.subject, &email, &permissions)
        .await
        .map_err(|e| invitation_error(&e, uri.path()))?;
    if mail
        .send_invitation(email.as_str(), &space.name, &issued.link())
        .await
        .is_err()
    {
        // A relay error invalidates the unsent link. The error response never exposes it.
        if let Err(error) = db
            .revoke_invitation(space_id, issued.invitation.id, principal.subject)
            .await
        {
            tracing::error!(?error, "failed to revoke invitation after SMTP failure");
        }
        return Err(ApiError::internal(uri.path()));
    }
    Ok((StatusCode::CREATED, Json(issued.invitation.into())))
}

#[utoipa::path(get, path = "/api/v1/spaces/{space_id}/invitations", tag = "sharing",
    responses((status = 200, body = InvitationListResponse), (status = 401, body = ProblemDetails), (status = 404, body = ProblemDetails)))]
async fn list_invitations(
    State(state): State<AppState>,
    uri: Uri,
    Path(space_id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<InvitationListResponse>, ApiError> {
    let db = state
        .database
        .as_deref()
        .ok_or_else(|| ApiError::service_unavailable(uri.path()))?;
    let principal = authenticated_headers(db, &headers, uri.path()).await?;
    let invitations = db
        .list_invitations(space_id, principal.subject)
        .await
        .map_err(|e| invitation_error(&e, uri.path()))?;
    Ok(Json(InvitationListResponse {
        invitations: invitations.into_iter().map(Into::into).collect(),
    }))
}

#[utoipa::path(delete, path = "/api/v1/spaces/{space_id}/invitations/{invitation_id}", tag = "sharing",
    responses((status = 204), (status = 401, body = ProblemDetails), (status = 404, body = ProblemDetails)))]
async fn revoke_invitation(
    State(state): State<AppState>,
    uri: Uri,
    Path((space_id, invitation_id)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let db = state
        .database
        .as_deref()
        .ok_or_else(|| ApiError::service_unavailable(uri.path()))?;
    let principal = authenticated_headers(db, &headers, uri.path()).await?;
    db.revoke_invitation(space_id, invitation_id, principal.subject)
        .await
        .map_err(|e| invitation_error(&e, uri.path()))?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(post, path = "/api/v1/invitations/{invitation_id}/accept", tag = "sharing",
    request_body = AcceptInvitationRequest,
    responses((status = 204), (status = 401, body = ProblemDetails), (status = 404, body = ProblemDetails), (status = 422, body = ProblemDetails)))]
async fn accept_invitation(
    State(state): State<AppState>,
    uri: Uri,
    Path(invitation_id): Path<Uuid>,
    headers: HeaderMap,
    Json(payload): Json<AcceptInvitationRequest>,
) -> Result<StatusCode, ApiError> {
    let db = state
        .database
        .as_deref()
        .ok_or_else(|| ApiError::service_unavailable(uri.path()))?;
    let token = InvitationToken::parse(payload.token)
        .map_err(|_| ApiError::resource_not_found(uri.path()))?;
    let signed_in = if headers.contains_key(SESSION_TOKEN_HEADER) {
        Some(
            authenticated_headers(db, &headers, uri.path())
                .await?
                .subject,
        )
    } else {
        None
    };
    let new_account = match (payload.display_name.as_deref(), payload.password.as_deref()) {
        (None, None) => None,
        (Some(name), Some(password)) => Some((name, password)),
        _ => {
            return Err(ApiError::invalid_request(
                uri.path(),
                "Nom et mot de passe sont requis ensemble.",
                "invalid_account",
            ));
        }
    };
    if signed_in.is_some() && new_account.is_some() {
        return Err(ApiError::invalid_request(
            uri.path(),
            "Un compte connecté ne doit pas fournir de nouveau mot de passe.",
            "invalid_account",
        ));
    }
    db.accept_invitation(
        invitation_id,
        &token,
        signed_in,
        new_account,
        &state.passwords,
    )
    .await
    .map_err(|e| invitation_error(&e, uri.path()))?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(get, path = "/api/v1/spaces/{space_id}/groups", tag = "collection",
    params(("space_id" = Uuid, Path, description = "Espace")),
    responses((status = 200, body = GroupListResponse), (status = 401, body = ProblemDetails),
        (status = 403, body = ProblemDetails)))]
async fn list_groups(
    State(state): State<AppState>,
    uri: Uri,
    Path(space_id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<GroupListResponse>, ApiError> {
    let db = state
        .database
        .as_deref()
        .ok_or_else(|| ApiError::service_unavailable(uri.path()))?;
    let principal = authenticated_headers(db, &headers, uri.path()).await?;
    authorized_space(
        db,
        &principal,
        space_id,
        Permission::CollectionsRead,
        uri.path(),
    )
    .await?;
    let groups = db
        .groups_in_space(space_id)
        .await
        .map_err(|error| group_error(&error, uri.path()))?;
    Ok(Json(GroupListResponse {
        groups: groups.into_iter().map(Into::into).collect(),
    }))
}

#[utoipa::path(post, path = "/api/v1/spaces/{space_id}/groups", tag = "collection",
    request_body = CreateGroupRequest,
    params(("space_id" = Uuid, Path, description = "Espace")),
    responses((status = 201, body = GroupResponse), (status = 401, body = ProblemDetails),
        (status = 403, body = ProblemDetails), (status = 409, body = ProblemDetails),
        (status = 422, body = ProblemDetails)))]
async fn create_group(
    State(state): State<AppState>,
    uri: Uri,
    Path(space_id): Path<Uuid>,
    headers: HeaderMap,
    Json(payload): Json<CreateGroupRequest>,
) -> Result<(StatusCode, Json<GroupResponse>), ApiError> {
    let db = state
        .database
        .as_deref()
        .ok_or_else(|| ApiError::service_unavailable(uri.path()))?;
    let principal = authenticated_headers(db, &headers, uri.path()).await?;
    authorized_space(
        db,
        &principal,
        space_id,
        Permission::CollectionsWrite,
        uri.path(),
    )
    .await?;
    let group = db
        .create_group(space_id, &payload.kind, &payload.name)
        .await
        .map_err(|error| group_error(&error, uri.path()))?;
    Ok((StatusCode::CREATED, Json(group.into())))
}

fn parse_group_revision(value: &str, instance: &str) -> Result<u64, ApiError> {
    value
        .parse::<u64>()
        .ok()
        .filter(|revision| *revision > 0)
        .ok_or_else(|| {
            ApiError::invalid_request(instance, "Revision invalide.", "invalid_revision")
        })
}

#[utoipa::path(patch, path = "/api/v1/spaces/{space_id}/groups/{group_id}", tag = "collection",
    request_body = RenameGroupRequest,
    params(("space_id" = Uuid, Path, description = "Espace"),
        ("group_id" = Uuid, Path, description = "Serie ou regroupement")),
    responses((status = 200, body = GroupResponse), (status = 401, body = ProblemDetails),
        (status = 403, body = ProblemDetails), (status = 404, body = ProblemDetails),
        (status = 409, body = ProblemDetails), (status = 422, body = ProblemDetails)))]
async fn rename_group(
    State(state): State<AppState>,
    uri: Uri,
    Path((space_id, group_id)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
    Json(payload): Json<RenameGroupRequest>,
) -> Result<Json<GroupResponse>, ApiError> {
    let db = state
        .database
        .as_deref()
        .ok_or_else(|| ApiError::service_unavailable(uri.path()))?;
    let principal = authenticated_headers(db, &headers, uri.path()).await?;
    authorized_space(
        db,
        &principal,
        space_id,
        Permission::CollectionsWrite,
        uri.path(),
    )
    .await?;
    let revision = parse_group_revision(&payload.expected_revision, uri.path())?;
    let group = db
        .rename_group(space_id, group_id, &payload.name, revision)
        .await
        .map_err(|error| group_error(&error, uri.path()))?;
    Ok(Json(group.into()))
}

#[utoipa::path(delete, path = "/api/v1/spaces/{space_id}/groups/{group_id}", tag = "collection",
    request_body = DeleteGroupRequest,
    params(("space_id" = Uuid, Path, description = "Espace"),
        ("group_id" = Uuid, Path, description = "Serie ou regroupement")),
    responses((status = 204, description = "Regroupement vide supprime"),
        (status = 401, body = ProblemDetails), (status = 403, body = ProblemDetails),
        (status = 404, body = ProblemDetails), (status = 409, body = ProblemDetails)))]
async fn delete_group(
    State(state): State<AppState>,
    uri: Uri,
    Path((space_id, group_id)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
    Json(payload): Json<DeleteGroupRequest>,
) -> Result<StatusCode, ApiError> {
    let db = state
        .database
        .as_deref()
        .ok_or_else(|| ApiError::service_unavailable(uri.path()))?;
    let principal = authenticated_headers(db, &headers, uri.path()).await?;
    authorized_space(
        db,
        &principal,
        space_id,
        Permission::CollectionsWrite,
        uri.path(),
    )
    .await?;
    let revision = parse_group_revision(&payload.expected_revision, uri.path())?;
    db.delete_group(space_id, group_id, revision)
        .await
        .map_err(|error| group_error(&error, uri.path()))?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(get, path = "/api/v1/items/{item_id}/groups", tag = "collection",
    params(("item_id" = Uuid, Path, description = "Objet")),
    responses((status = 200, body = ItemGroupListResponse), (status = 401, body = ProblemDetails),
        (status = 403, body = ProblemDetails), (status = 404, body = ProblemDetails)))]
async fn list_item_groups(
    State(state): State<AppState>,
    uri: Uri,
    Path(item_id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<ItemGroupListResponse>, ApiError> {
    let db = state
        .database
        .as_deref()
        .ok_or_else(|| ApiError::service_unavailable(uri.path()))?;
    let principal = authenticated_headers(db, &headers, uri.path()).await?;
    let item = db
        .item(item_id)
        .await
        .map_err(|error| match error.inventory_error() {
            Some(InventoryError::ItemNotFound) => ApiError::resource_not_found(uri.path()),
            _ => ApiError::internal(uri.path()),
        })?;
    authorized_space(
        db,
        &principal,
        item.space_id,
        Permission::CollectionsRead,
        uri.path(),
    )
    .await?;
    let groups = db
        .item_groups(item_id)
        .await
        .map_err(|error| group_error(&error, uri.path()))?;
    Ok(Json(ItemGroupListResponse {
        revision: item.revision.to_string(),
        groups: groups.into_iter().map(Into::into).collect(),
    }))
}

#[utoipa::path(put, path = "/api/v1/items/{item_id}/groups", tag = "collection",
    request_body = ReplaceItemGroupsRequest,
    params(("item_id" = Uuid, Path, description = "Objet")),
    responses((status = 200, body = ItemGroupListResponse), (status = 401, body = ProblemDetails),
        (status = 403, body = ProblemDetails), (status = 404, body = ProblemDetails),
        (status = 409, body = ProblemDetails), (status = 422, body = ProblemDetails)))]
async fn replace_item_groups(
    State(state): State<AppState>,
    uri: Uri,
    Path(item_id): Path<Uuid>,
    headers: HeaderMap,
    Json(payload): Json<ReplaceItemGroupsRequest>,
) -> Result<Json<ItemGroupListResponse>, ApiError> {
    let db = state
        .database
        .as_deref()
        .ok_or_else(|| ApiError::service_unavailable(uri.path()))?;
    let principal = authenticated_headers(db, &headers, uri.path()).await?;
    let item = db
        .item(item_id)
        .await
        .map_err(|error| match error.inventory_error() {
            Some(InventoryError::ItemNotFound) => ApiError::resource_not_found(uri.path()),
            _ => ApiError::internal(uri.path()),
        })?;
    authorized_space(
        db,
        &principal,
        item.space_id,
        Permission::CollectionsWrite,
        uri.path(),
    )
    .await?;
    let expected_revision = payload
        .expected_revision
        .parse::<u64>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| {
            ApiError::invalid_request(uri.path(), "Revision invalide.", "invalid_revision")
        })?;
    if payload.group_ids.len() > 50 {
        return Err(ApiError::invalid_request(
            uri.path(),
            "Choisissez au plus 50 regroupements.",
            "invalid_groups",
        ));
    }
    let revision = db
        .replace_item_groups(item_id, &payload.group_ids, expected_revision)
        .await
        .map_err(|error| group_error(&error, uri.path()))?;
    let groups = db
        .item_groups(item_id)
        .await
        .map_err(|error| group_error(&error, uri.path()))?;
    Ok(Json(ItemGroupListResponse {
        revision: revision.to_string(),
        groups: groups.into_iter().map(Into::into).collect(),
    }))
}

#[utoipa::path(get, path = "/api/v1/items/{item_id}/relations", tag = "collection",
    params(("item_id" = Uuid, Path, description = "Objet")),
    responses((status = 200, body = ItemRelationListResponse), (status = 401, body = ProblemDetails),
        (status = 403, body = ProblemDetails), (status = 404, body = ProblemDetails)))]
async fn list_item_relations(
    State(state): State<AppState>,
    uri: Uri,
    Path(item_id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<ItemRelationListResponse>, ApiError> {
    let db = state
        .database
        .as_deref()
        .ok_or_else(|| ApiError::service_unavailable(uri.path()))?;
    let principal = authenticated_headers(db, &headers, uri.path()).await?;
    let item = db
        .item(item_id)
        .await
        .map_err(|error| match error.inventory_error() {
            Some(InventoryError::ItemNotFound) => ApiError::resource_not_found(uri.path()),
            _ => ApiError::internal(uri.path()),
        })?;
    authorized_space(
        db,
        &principal,
        item.space_id,
        Permission::CollectionsRead,
        uri.path(),
    )
    .await?;
    let relations = db
        .item_relations(item_id)
        .await
        .map_err(|error| relation_error(&error, uri.path()))?;
    Ok(Json(ItemRelationListResponse {
        revision: item.revision.to_string(),
        relations: relations
            .into_iter()
            .map(|relation| ItemRelationResponse::from_relation(relation, item_id))
            .collect(),
    }))
}

#[utoipa::path(put, path = "/api/v1/items/{item_id}/relations", tag = "collection",
    request_body = ReplaceItemRelationsRequest,
    params(("item_id" = Uuid, Path, description = "Objet")),
    responses((status = 200, body = ItemRelationListResponse), (status = 401, body = ProblemDetails),
        (status = 403, body = ProblemDetails), (status = 404, body = ProblemDetails),
        (status = 409, body = ProblemDetails), (status = 422, body = ProblemDetails)))]
async fn replace_item_relations(
    State(state): State<AppState>,
    uri: Uri,
    Path(item_id): Path<Uuid>,
    headers: HeaderMap,
    Json(payload): Json<ReplaceItemRelationsRequest>,
) -> Result<Json<ItemRelationListResponse>, ApiError> {
    let db = state
        .database
        .as_deref()
        .ok_or_else(|| ApiError::service_unavailable(uri.path()))?;
    let principal = authenticated_headers(db, &headers, uri.path()).await?;
    let item = db
        .item(item_id)
        .await
        .map_err(|error| match error.inventory_error() {
            Some(InventoryError::ItemNotFound) => ApiError::resource_not_found(uri.path()),
            _ => ApiError::internal(uri.path()),
        })?;
    authorized_space(
        db,
        &principal,
        item.space_id,
        Permission::CollectionsWrite,
        uri.path(),
    )
    .await?;
    let expected_revision = payload
        .expected_revision
        .parse::<u64>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| {
            ApiError::invalid_request(uri.path(), "Revision invalide.", "invalid_revision")
        })?;
    if payload.relations.len() > MAX_ITEM_RELATIONS {
        return Err(ApiError::invalid_request(
            uri.path(),
            "Choisissez au plus 50 relations.",
            "invalid_relations",
        ));
    }
    let mut entries = Vec::with_capacity(payload.relations.len());
    for input in &payload.relations {
        let kind = RelationKind::from_code(&input.kind).ok_or_else(|| {
            ApiError::invalid_request(
                uri.path(),
                "Type de relation invalide.",
                "invalid_relation_kind",
            )
        })?;
        entries.push((input.target_id, kind));
    }
    let revision = db
        .replace_item_relations(item_id, expected_revision, &entries, principal.subject)
        .await
        .map_err(|error| relation_error(&error, uri.path()))?;
    let relations = db
        .item_relations(item_id)
        .await
        .map_err(|error| relation_error(&error, uri.path()))?;
    Ok(Json(ItemRelationListResponse {
        revision: revision.to_string(),
        relations: relations
            .into_iter()
            .map(|relation| ItemRelationResponse::from_relation(relation, item_id))
            .collect(),
    }))
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ItemResponse {
    pub id: Uuid,
    pub space_id: Uuid,
    pub inventory_number: String,
    pub name: String,
    pub description: Option<String>,
    pub historical_reference: Option<String>,
    pub technical_reference: Option<String>,
    pub state: String,
    pub revision: String,
    pub created_at: String,
}
impl From<Item> for ItemResponse {
    fn from(value: Item) -> Self {
        Self {
            id: value.id,
            space_id: value.space_id,
            inventory_number: value.inventory_number.to_string(),
            name: value.name,
            description: value.description,
            historical_reference: value.historical_reference,
            technical_reference: value.technical_reference,
            state: value.state.as_str().to_owned(),
            revision: value.revision.to_string(),
            created_at: value.created_at.to_rfc3339(),
        }
    }
}
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ItemListResponse {
    pub items: Vec<ItemResponse>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ListItemsQuery {
    q: Option<String>,
    after: Option<String>,
    limit: Option<String>,
    state: Option<String>,
    category_id: Option<String>,
    location_id: Option<String>,
    group_id: Option<String>,
}
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CreateItemRequest {
    pub name: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ItemStateRequest {
    pub expected_revision: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct RenameItemRequest {
    pub name: String,
    pub expected_revision: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct UpdateItemDetailsRequest {
    pub description: Option<String>,
    pub historical_reference: Option<String>,
    pub technical_reference: Option<String>,
    pub expected_revision: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct TransferItemRequest {
    pub destination_space_id: Uuid,
    pub expected_revision: String,
    pub destination_category_ids: Option<Vec<Uuid>>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ItemTransferResponse {
    pub id: Uuid,
    pub source_space_id: Uuid,
    pub destination_space_id: Uuid,
    pub source_inventory_number: String,
    pub destination_inventory_number: String,
    pub transferred_at: String,
}

impl From<ItemTransfer> for ItemTransferResponse {
    fn from(value: ItemTransfer) -> Self {
        Self {
            id: value.id,
            source_space_id: value.source_space_id,
            destination_space_id: value.destination_space_id,
            source_inventory_number: value.source_inventory_number.to_string(),
            destination_inventory_number: value.destination_inventory_number.to_string(),
            transferred_at: value.transferred_at.to_rfc3339(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct TransferOutcomeResponse {
    pub item: ItemResponse,
    pub transfer: ItemTransferResponse,
}

impl From<TransferOutcome> for TransferOutcomeResponse {
    fn from(value: TransferOutcome) -> Self {
        Self {
            item: value.item.into(),
            transfer: value.transfer.into(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ItemTransferListResponse {
    pub transfers: Vec<ItemTransferResponse>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ItemStateEventResponse {
    pub id: Uuid,
    pub item_id: Uuid,
    pub space_id: Uuid,
    pub actor_account_id: Uuid,
    pub actor_display_name: String,
    pub state_before: String,
    pub state_after: String,
    pub created_at: String,
}

impl From<ItemStateEvent> for ItemStateEventResponse {
    fn from(value: ItemStateEvent) -> Self {
        Self {
            id: value.id,
            item_id: value.item_id,
            space_id: value.space_id,
            actor_account_id: value.actor_account_id,
            actor_display_name: value.actor_display_name,
            state_before: value.state_before.as_str().to_owned(),
            state_after: value.state_after.as_str().to_owned(),
            created_at: value.created_at.to_rfc3339(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ItemStateEventListResponse {
    pub events: Vec<ItemStateEventResponse>,
}

#[utoipa::path(get, path = "/api/v1/spaces", tag = "collection",
    responses((status = 200, body = SpaceListResponse), (status = 401, body = ProblemDetails)))]
async fn list_spaces(
    State(state): State<AppState>,
    uri: Uri,
    headers: HeaderMap,
) -> Result<Json<SpaceListResponse>, ApiError> {
    let database = state
        .database
        .as_deref()
        .ok_or_else(|| ApiError::service_unavailable(uri.path()))?;
    let principal = authenticated_headers(database, &headers, uri.path()).await?;
    let spaces = database
        .spaces_for_account(principal.subject)
        .await
        .map_err(|_| ApiError::internal(uri.path()))?;
    let mut visible = Vec::new();
    for space in spaces {
        let membership = database
            .membership(space.id, principal.subject)
            .await
            .map_err(|_| ApiError::internal(uri.path()))?;
        let membership = membership
            .as_ref()
            .map(crate::database::Membership::as_space_membership);
        if [Permission::CollectionsRead, Permission::AcquisitionsRead]
            .into_iter()
            .any(|permission| {
                principal
                    .require_space_permission(space.id, membership.as_ref(), permission)
                    .is_ok()
            })
        {
            visible.push(space.into());
        }
    }
    Ok(Json(SpaceListResponse { spaces: visible }))
}

#[utoipa::path(get, path = "/api/v1/spaces/{space_id}/my-permissions", tag = "sharing",
    params(("space_id" = Uuid, Path, description = "Espace")),
    responses((status = 200, body = SpacePermissionsResponse), (status = 401, body = ProblemDetails),
        (status = 404, body = ProblemDetails)))]
async fn my_space_permissions(
    State(state): State<AppState>,
    uri: Uri,
    Path(space_id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<SpacePermissionsResponse>, ApiError> {
    let db = state
        .database
        .as_deref()
        .ok_or_else(|| ApiError::service_unavailable(uri.path()))?;
    let principal = authenticated_headers(db, &headers, uri.path()).await?;
    let membership = db
        .membership(space_id, principal.subject)
        .await
        .map_err(|_| ApiError::internal(uri.path()))?
        .ok_or_else(|| ApiError::resource_not_found(uri.path()))?;
    Ok(Json(SpacePermissionsResponse {
        permissions: membership.permissions.into_iter().collect(),
    }))
}

async fn acquisitions_db<'a>(
    state: &'a AppState,
    uri: &Uri,
    headers: &HeaderMap,
    space_id: Uuid,
    permission: Permission,
) -> Result<&'a Database, ApiError> {
    let db = state
        .database
        .as_deref()
        .ok_or_else(|| ApiError::service_unavailable(uri.path()))?;
    let principal = authenticated_headers(db, headers, uri.path()).await?;
    authorized_space(db, &principal, space_id, permission, uri.path()).await?;
    Ok(db)
}

#[utoipa::path(get, path = "/api/v1/spaces/{space_id}/wishes", tag = "acquisitions",
    params(("space_id" = Uuid, Path, description = "Espace")),
    responses((status = 200, body = WishListResponse), (status = 401, body = ProblemDetails),
        (status = 403, body = ProblemDetails), (status = 404, body = ProblemDetails)))]
async fn list_wishes(
    State(state): State<AppState>,
    uri: Uri,
    Path(space_id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<WishListResponse>, ApiError> {
    let db = acquisitions_db(
        &state,
        &uri,
        &headers,
        space_id,
        Permission::AcquisitionsRead,
    )
    .await?;
    let wishes = db
        .wishes_in_space(space_id)
        .await
        .map_err(|_| ApiError::internal(uri.path()))?;
    Ok(Json(WishListResponse {
        wishes: wishes.into_iter().map(Into::into).collect(),
    }))
}

#[utoipa::path(post, path = "/api/v1/spaces/{space_id}/wishes", tag = "acquisitions",
    request_body = CreateWishRequest,
    params(("space_id" = Uuid, Path, description = "Espace")),
    responses((status = 201, body = WishResponse), (status = 401, body = ProblemDetails),
        (status = 403, body = ProblemDetails), (status = 422, body = ProblemDetails)))]
async fn create_wish(
    State(state): State<AppState>,
    uri: Uri,
    Path(space_id): Path<Uuid>,
    headers: HeaderMap,
    Json(payload): Json<CreateWishRequest>,
) -> Result<(StatusCode, Json<WishResponse>), ApiError> {
    let db = acquisitions_db(
        &state,
        &uri,
        &headers,
        space_id,
        Permission::AcquisitionsWrite,
    )
    .await?;
    let wish = db
        .create_wish(space_id, &payload.title, payload.search_notes.as_deref())
        .await
        .map_err(|error| acquisition_error(&error, uri.path()))?;
    Ok((StatusCode::CREATED, Json(wish.into())))
}

#[utoipa::path(patch, path = "/api/v1/spaces/{space_id}/wishes/{wish_id}", tag = "acquisitions",
    request_body = UpdateWishRequest,
    params(("space_id" = Uuid, Path, description = "Espace"),
        ("wish_id" = Uuid, Path, description = "Envie")),
    responses((status = 200, body = WishResponse), (status = 401, body = ProblemDetails),
        (status = 403, body = ProblemDetails), (status = 404, body = ProblemDetails),
        (status = 409, body = ProblemDetails), (status = 422, body = ProblemDetails)))]
async fn update_wish(
    State(state): State<AppState>,
    uri: Uri,
    Path((space_id, wish_id)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
    Json(payload): Json<UpdateWishRequest>,
) -> Result<Json<WishResponse>, ApiError> {
    let db = acquisitions_db(
        &state,
        &uri,
        &headers,
        space_id,
        Permission::AcquisitionsWrite,
    )
    .await?;
    let revision = parse_group_revision(&payload.expected_revision, uri.path())?;
    let wish = db
        .update_wish(
            space_id,
            wish_id,
            &payload.title,
            payload.search_notes.as_deref(),
            revision,
        )
        .await
        .map_err(|error| acquisition_error(&error, uri.path()))?;
    Ok(Json(wish.into()))
}

#[utoipa::path(get, path = "/api/v1/spaces/{space_id}/vendors", tag = "acquisitions",
    params(("space_id" = Uuid, Path, description = "Espace")),
    responses((status = 200, body = VendorListResponse), (status = 401, body = ProblemDetails),
        (status = 403, body = ProblemDetails), (status = 404, body = ProblemDetails)))]
async fn list_vendors(
    State(state): State<AppState>,
    uri: Uri,
    Path(space_id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<VendorListResponse>, ApiError> {
    let db = acquisitions_db(
        &state,
        &uri,
        &headers,
        space_id,
        Permission::AcquisitionsRead,
    )
    .await?;
    let vendors = db
        .vendors_in_space(space_id)
        .await
        .map_err(|_| ApiError::internal(uri.path()))?;
    Ok(Json(VendorListResponse {
        vendors: vendors.into_iter().map(Into::into).collect(),
    }))
}

#[utoipa::path(post, path = "/api/v1/spaces/{space_id}/vendors", tag = "acquisitions",
    request_body = CreateVendorRequest,
    params(("space_id" = Uuid, Path, description = "Espace")),
    responses((status = 201, body = VendorResponse), (status = 401, body = ProblemDetails),
        (status = 403, body = ProblemDetails), (status = 409, body = ProblemDetails),
        (status = 422, body = ProblemDetails)))]
async fn create_vendor(
    State(state): State<AppState>,
    uri: Uri,
    Path(space_id): Path<Uuid>,
    headers: HeaderMap,
    Json(payload): Json<CreateVendorRequest>,
) -> Result<(StatusCode, Json<VendorResponse>), ApiError> {
    let db = acquisitions_db(
        &state,
        &uri,
        &headers,
        space_id,
        Permission::AcquisitionsWrite,
    )
    .await?;
    let vendor = db
        .create_vendor(space_id, &payload.name, payload.website_url.as_deref())
        .await
        .map_err(|error| acquisition_error(&error, uri.path()))?;
    Ok((StatusCode::CREATED, Json(vendor.into())))
}

#[utoipa::path(patch, path = "/api/v1/spaces/{space_id}/vendors/{vendor_id}", tag = "acquisitions",
    request_body = UpdateVendorRequest,
    params(("space_id" = Uuid, Path, description = "Espace"),
        ("vendor_id" = Uuid, Path, description = "Vendeur")),
    responses((status = 200, body = VendorResponse), (status = 401, body = ProblemDetails),
        (status = 403, body = ProblemDetails), (status = 404, body = ProblemDetails),
        (status = 409, body = ProblemDetails), (status = 422, body = ProblemDetails)))]
async fn update_vendor(
    State(state): State<AppState>,
    uri: Uri,
    Path((space_id, vendor_id)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
    Json(payload): Json<UpdateVendorRequest>,
) -> Result<Json<VendorResponse>, ApiError> {
    let db = acquisitions_db(
        &state,
        &uri,
        &headers,
        space_id,
        Permission::AcquisitionsWrite,
    )
    .await?;
    let revision = parse_group_revision(&payload.expected_revision, uri.path())?;
    let vendor = db
        .update_vendor(
            space_id,
            vendor_id,
            &payload.name,
            payload.website_url.as_deref(),
            revision,
        )
        .await
        .map_err(|error| acquisition_error(&error, uri.path()))?;
    Ok(Json(vendor.into()))
}

#[utoipa::path(get, path = "/api/v1/spaces/{space_id}/wishes/{wish_id}/offers", tag = "acquisitions",
    params(("space_id" = Uuid, Path, description = "Espace"),
        ("wish_id" = Uuid, Path, description = "Envie")),
    responses((status = 200, body = OfferListResponse), (status = 401, body = ProblemDetails),
        (status = 403, body = ProblemDetails), (status = 404, body = ProblemDetails)))]
async fn list_offers(
    State(state): State<AppState>,
    uri: Uri,
    Path((space_id, wish_id)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
) -> Result<Json<OfferListResponse>, ApiError> {
    let db = acquisitions_db(
        &state,
        &uri,
        &headers,
        space_id,
        Permission::AcquisitionsRead,
    )
    .await?;
    let offers = db
        .offers_for_wish(space_id, wish_id)
        .await
        .map_err(|error| acquisition_error(&error, uri.path()))?;
    Ok(Json(OfferListResponse {
        offers: offers.into_iter().map(Into::into).collect(),
    }))
}

#[utoipa::path(post, path = "/api/v1/spaces/{space_id}/wishes/{wish_id}/offers", tag = "acquisitions",
    request_body = CreateOfferRequest,
    params(("space_id" = Uuid, Path, description = "Espace"),
        ("wish_id" = Uuid, Path, description = "Envie")),
    responses((status = 201, body = OfferResponse), (status = 401, body = ProblemDetails),
        (status = 403, body = ProblemDetails), (status = 404, body = ProblemDetails),
        (status = 422, body = ProblemDetails)))]
async fn create_offer(
    State(state): State<AppState>,
    uri: Uri,
    Path((space_id, wish_id)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
    Json(payload): Json<CreateOfferRequest>,
) -> Result<(StatusCode, Json<OfferResponse>), ApiError> {
    let db = acquisitions_db(
        &state,
        &uri,
        &headers,
        space_id,
        Permission::AcquisitionsWrite,
    )
    .await?;
    let offer = db
        .create_offer(
            space_id,
            wish_id,
            payload.vendor_id,
            &payload.title,
            payload.source_url.as_deref(),
            payload.notes.as_deref(),
        )
        .await
        .map_err(|error| acquisition_error(&error, uri.path()))?;
    Ok((StatusCode::CREATED, Json(offer.into())))
}

#[utoipa::path(patch, path = "/api/v1/spaces/{space_id}/wishes/{wish_id}/offers/{offer_id}", tag = "acquisitions",
    request_body = UpdateOfferRequest,
    params(("space_id" = Uuid, Path, description = "Espace"),
        ("wish_id" = Uuid, Path, description = "Envie"),
        ("offer_id" = Uuid, Path, description = "Offre")),
    responses((status = 200, body = OfferResponse), (status = 401, body = ProblemDetails),
        (status = 403, body = ProblemDetails), (status = 404, body = ProblemDetails),
        (status = 409, body = ProblemDetails), (status = 422, body = ProblemDetails)))]
async fn update_offer(
    State(state): State<AppState>,
    uri: Uri,
    Path((space_id, wish_id, offer_id)): Path<(Uuid, Uuid, Uuid)>,
    headers: HeaderMap,
    Json(payload): Json<UpdateOfferRequest>,
) -> Result<Json<OfferResponse>, ApiError> {
    let db = acquisitions_db(
        &state,
        &uri,
        &headers,
        space_id,
        Permission::AcquisitionsWrite,
    )
    .await?;
    let revision = parse_group_revision(&payload.expected_revision, uri.path())?;
    let offer = db
        .update_offer(
            space_id,
            wish_id,
            offer_id,
            payload.vendor_id,
            &payload.title,
            payload.source_url.as_deref(),
            payload.notes.as_deref(),
            revision,
        )
        .await
        .map_err(|error| acquisition_error(&error, uri.path()))?;
    Ok(Json(offer.into()))
}

#[utoipa::path(get, path = "/api/v1/admin/spaces", tag = "sharing",
    responses((status = 200, body = SpaceListResponse), (status = 401, body = ProblemDetails), (status = 404, body = ProblemDetails)))]
async fn list_admin_spaces(
    State(state): State<AppState>,
    uri: Uri,
    headers: HeaderMap,
) -> Result<Json<SpaceListResponse>, ApiError> {
    let db = state
        .database
        .as_deref()
        .ok_or_else(|| ApiError::service_unavailable(uri.path()))?;
    let principal = authenticated_headers(db, &headers, uri.path()).await?;
    if !principal.roles.contains("system_admin") {
        return Err(ApiError::resource_not_found(uri.path()));
    }
    let spaces = db
        .all_spaces()
        .await
        .map_err(|_| ApiError::internal(uri.path()))?;
    Ok(Json(SpaceListResponse {
        spaces: spaces.into_iter().map(Into::into).collect(),
    }))
}

#[utoipa::path(post, path = "/api/v1/spaces", tag = "collection", request_body = CreateSpaceRequest,
    responses((status = 201, body = SpaceResponse), (status = 401, body = ProblemDetails), (status = 422, body = ProblemDetails)))]
async fn create_space(
    State(state): State<AppState>,
    uri: Uri,
    headers: HeaderMap,
    Json(payload): Json<CreateSpaceRequest>,
) -> Result<(StatusCode, Json<SpaceResponse>), ApiError> {
    let database = state
        .database
        .as_deref()
        .ok_or_else(|| ApiError::service_unavailable(uri.path()))?;
    let principal = authenticated_headers(database, &headers, uri.path()).await?;
    let space = database
        .create_space(&payload.name, principal.subject)
        .await
        .map_err(|error| match error.space_error() {
            Some(SpaceError::InvalidName) => {
                ApiError::invalid_request(uri.path(), "Nom d'espace invalide.", "invalid_name")
            }
            _ => ApiError::internal(uri.path()),
        })?;
    Ok((StatusCode::CREATED, Json(space.into())))
}

#[utoipa::path(get, path = "/api/v1/spaces/{space_id}/categories", tag = "collection",
    params(("space_id" = Uuid, Path, description = "Espace")),
    responses((status = 200, body = CategoryListResponse), (status = 401, body = ProblemDetails),
        (status = 403, body = ProblemDetails), (status = 404, body = ProblemDetails)))]
async fn list_categories(
    State(state): State<AppState>,
    uri: Uri,
    Path(space_id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<CategoryListResponse>, ApiError> {
    let db = state
        .database
        .as_deref()
        .ok_or_else(|| ApiError::service_unavailable(uri.path()))?;
    let principal = authenticated_headers(db, &headers, uri.path()).await?;
    authorized_space(
        db,
        &principal,
        space_id,
        Permission::CollectionsRead,
        uri.path(),
    )
    .await?;
    let categories = db
        .categories_in_space(space_id)
        .await
        .map_err(|_| ApiError::internal(uri.path()))?;
    Ok(Json(CategoryListResponse {
        categories: categories.into_iter().map(Into::into).collect(),
    }))
}

#[utoipa::path(post, path = "/api/v1/spaces/{space_id}/categories", tag = "collection", request_body = CreateCategoryRequest,
    params(("space_id" = Uuid, Path, description = "Espace")),
    responses((status = 201, body = CategoryResponse), (status = 401, body = ProblemDetails),
        (status = 403, body = ProblemDetails), (status = 404, body = ProblemDetails),
        (status = 409, body = ProblemDetails), (status = 422, body = ProblemDetails)))]
async fn create_category(
    State(state): State<AppState>,
    uri: Uri,
    Path(space_id): Path<Uuid>,
    headers: HeaderMap,
    Json(payload): Json<CreateCategoryRequest>,
) -> Result<(StatusCode, Json<CategoryResponse>), ApiError> {
    let db = state
        .database
        .as_deref()
        .ok_or_else(|| ApiError::service_unavailable(uri.path()))?;
    let principal = authenticated_headers(db, &headers, uri.path()).await?;
    authorized_space(
        db,
        &principal,
        space_id,
        Permission::CollectionsWrite,
        uri.path(),
    )
    .await?;
    let category = db
        .create_category(space_id, payload.parent_id, &payload.name)
        .await
        .map_err(|error| taxonomy_error(&error, uri.path()))?;
    Ok((StatusCode::CREATED, Json(category.into())))
}

#[utoipa::path(get, path = "/api/v1/spaces/{space_id}/categories/{category_id}/fields", tag = "collection",
    params(("space_id" = Uuid, Path, description = "Espace"), ("category_id" = Uuid, Path, description = "Categorie")),
    responses((status = 200, body = CategoryFieldListResponse), (status = 401, body = ProblemDetails),
        (status = 403, body = ProblemDetails), (status = 404, body = ProblemDetails)))]
async fn list_category_fields(
    State(state): State<AppState>,
    uri: Uri,
    Path((space_id, category_id)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
) -> Result<Json<CategoryFieldListResponse>, ApiError> {
    let db = state
        .database
        .as_deref()
        .ok_or_else(|| ApiError::service_unavailable(uri.path()))?;
    let principal = authenticated_headers(db, &headers, uri.path()).await?;
    authorized_space(
        db,
        &principal,
        space_id,
        Permission::CollectionsRead,
        uri.path(),
    )
    .await?;
    let fields = db
        .category_fields(space_id, category_id)
        .await
        .map_err(|error| taxonomy_error(&error, uri.path()))?;
    Ok(Json(CategoryFieldListResponse {
        fields: fields.into_iter().map(Into::into).collect(),
    }))
}

#[utoipa::path(post, path = "/api/v1/spaces/{space_id}/categories/{category_id}/fields", tag = "collection", request_body = CreateCategoryFieldRequest,
    params(("space_id" = Uuid, Path, description = "Espace"), ("category_id" = Uuid, Path, description = "Categorie")),
    responses((status = 201, body = CategoryFieldResponse), (status = 401, body = ProblemDetails),
        (status = 403, body = ProblemDetails), (status = 404, body = ProblemDetails),
        (status = 409, body = ProblemDetails), (status = 422, body = ProblemDetails)))]
async fn create_category_field(
    State(state): State<AppState>,
    uri: Uri,
    Path((space_id, category_id)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
    Json(payload): Json<CreateCategoryFieldRequest>,
) -> Result<(StatusCode, Json<CategoryFieldResponse>), ApiError> {
    let db = state
        .database
        .as_deref()
        .ok_or_else(|| ApiError::service_unavailable(uri.path()))?;
    let principal = authenticated_headers(db, &headers, uri.path()).await?;
    authorized_space(
        db,
        &principal,
        space_id,
        Permission::CollectionsWrite,
        uri.path(),
    )
    .await?;
    let value_type = FieldType::from_code(&payload.value_type).ok_or_else(|| {
        ApiError::invalid_request(uri.path(), "Type de champ invalide.", "invalid_field_type")
    })?;
    let field = db
        .create_category_field(space_id, category_id, &payload.name, value_type)
        .await
        .map_err(|error| taxonomy_error(&error, uri.path()))?;
    Ok((StatusCode::CREATED, Json(field.into())))
}

#[utoipa::path(patch, path = "/api/v1/spaces/{space_id}/categories/{category_id}", tag = "collection", request_body = RenameDefinitionRequest,
    params(("space_id" = Uuid, Path, description = "Espace"), ("category_id" = Uuid, Path, description = "Categorie")),
    responses((status = 200, body = CategoryResponse), (status = 401, body = ProblemDetails),
        (status = 403, body = ProblemDetails), (status = 404, body = ProblemDetails),
        (status = 409, body = ProblemDetails), (status = 422, body = ProblemDetails)))]
async fn rename_category(
    State(state): State<AppState>,
    uri: Uri,
    Path((space_id, category_id)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
    Json(payload): Json<RenameDefinitionRequest>,
) -> Result<Json<CategoryResponse>, ApiError> {
    let db = state
        .database
        .as_deref()
        .ok_or_else(|| ApiError::service_unavailable(uri.path()))?;
    let principal = authenticated_headers(db, &headers, uri.path()).await?;
    authorized_space(
        db,
        &principal,
        space_id,
        Permission::CollectionsWrite,
        uri.path(),
    )
    .await?;
    let category = db
        .rename_category(space_id, category_id, &payload.name)
        .await
        .map_err(|error| taxonomy_error(&error, uri.path()))?;
    Ok(Json(category.into()))
}

#[utoipa::path(patch, path = "/api/v1/spaces/{space_id}/categories/{category_id}/fields/{field_id}", tag = "collection", request_body = RenameDefinitionRequest,
    params(("space_id" = Uuid, Path, description = "Espace"), ("category_id" = Uuid, Path, description = "Categorie"), ("field_id" = Uuid, Path, description = "Champ")),
    responses((status = 200, body = CategoryFieldResponse), (status = 401, body = ProblemDetails),
        (status = 403, body = ProblemDetails), (status = 404, body = ProblemDetails),
        (status = 409, body = ProblemDetails), (status = 422, body = ProblemDetails)))]
async fn rename_category_field(
    State(state): State<AppState>,
    uri: Uri,
    Path((space_id, category_id, field_id)): Path<(Uuid, Uuid, Uuid)>,
    headers: HeaderMap,
    Json(payload): Json<RenameDefinitionRequest>,
) -> Result<Json<CategoryFieldResponse>, ApiError> {
    let db = state
        .database
        .as_deref()
        .ok_or_else(|| ApiError::service_unavailable(uri.path()))?;
    let principal = authenticated_headers(db, &headers, uri.path()).await?;
    authorized_space(
        db,
        &principal,
        space_id,
        Permission::CollectionsWrite,
        uri.path(),
    )
    .await?;
    let field = db
        .rename_category_field(space_id, category_id, field_id, &payload.name)
        .await
        .map_err(|error| taxonomy_error(&error, uri.path()))?;
    Ok(Json(field.into()))
}

#[utoipa::path(get, path = "/api/v1/spaces/{space_id}/locations", tag = "collection",
    params(("space_id" = Uuid, Path, description = "Espace")),
    responses((status = 200, body = LocationListResponse), (status = 401, body = ProblemDetails),
        (status = 403, body = ProblemDetails), (status = 404, body = ProblemDetails)))]
async fn list_locations(
    State(state): State<AppState>,
    uri: Uri,
    Path(space_id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<LocationListResponse>, ApiError> {
    let db = state
        .database
        .as_deref()
        .ok_or_else(|| ApiError::service_unavailable(uri.path()))?;
    let principal = authenticated_headers(db, &headers, uri.path()).await?;
    authorized_space(
        db,
        &principal,
        space_id,
        Permission::CollectionsRead,
        uri.path(),
    )
    .await?;
    let locations = db
        .locations_in_space(space_id)
        .await
        .map_err(|error| location_error(&error, uri.path()))?;
    Ok(Json(LocationListResponse {
        locations: locations.into_iter().map(Into::into).collect(),
    }))
}

#[utoipa::path(post, path = "/api/v1/spaces/{space_id}/locations", tag = "collection", request_body = CreateLocationRequest,
    params(("space_id" = Uuid, Path, description = "Espace")),
    responses((status = 201, body = LocationResponse), (status = 401, body = ProblemDetails),
        (status = 403, body = ProblemDetails), (status = 404, body = ProblemDetails),
        (status = 409, body = ProblemDetails), (status = 422, body = ProblemDetails)))]
async fn create_location(
    State(state): State<AppState>,
    uri: Uri,
    Path(space_id): Path<Uuid>,
    headers: HeaderMap,
    Json(payload): Json<CreateLocationRequest>,
) -> Result<(StatusCode, Json<LocationResponse>), ApiError> {
    let db = state
        .database
        .as_deref()
        .ok_or_else(|| ApiError::service_unavailable(uri.path()))?;
    let principal = authenticated_headers(db, &headers, uri.path()).await?;
    authorized_space(
        db,
        &principal,
        space_id,
        Permission::CollectionsWrite,
        uri.path(),
    )
    .await?;
    let location = db
        .create_location(space_id, payload.parent_id, &payload.name)
        .await
        .map_err(|error| location_error(&error, uri.path()))?;
    Ok((StatusCode::CREATED, Json(location.into())))
}

#[utoipa::path(get, path = "/api/v1/items/{item_id}/location", tag = "collection",
    params(("item_id" = Uuid, Path, description = "Objet")),
    responses((status = 200, body = ItemLocationResponse), (status = 401, body = ProblemDetails),
        (status = 403, body = ProblemDetails), (status = 404, body = ProblemDetails)))]
async fn get_item_location(
    State(state): State<AppState>,
    uri: Uri,
    Path(item_id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<ItemLocationResponse>, ApiError> {
    let db = state
        .database
        .as_deref()
        .ok_or_else(|| ApiError::service_unavailable(uri.path()))?;
    let principal = authenticated_headers(db, &headers, uri.path()).await?;
    let item = authorized_item(
        db,
        &principal,
        item_id,
        Permission::CollectionsRead,
        uri.path(),
    )
    .await?;
    let location = db
        .current_item_location(item_id)
        .await
        .map_err(|error| location_error(&error, uri.path()))?;
    Ok(Json(ItemLocationResponse {
        revision: item.revision.to_string(),
        location: location.map(Into::into),
    }))
}

#[utoipa::path(put, path = "/api/v1/items/{item_id}/location", tag = "collection", request_body = MoveItemLocationRequest,
    params(("item_id" = Uuid, Path, description = "Objet")),
    responses((status = 200, body = ItemLocationResponse), (status = 401, body = ProblemDetails),
        (status = 403, body = ProblemDetails), (status = 404, body = ProblemDetails),
        (status = 409, body = ProblemDetails), (status = 422, body = ProblemDetails)))]
async fn move_item_location(
    State(state): State<AppState>,
    uri: Uri,
    Path(item_id): Path<Uuid>,
    headers: HeaderMap,
    Json(payload): Json<MoveItemLocationRequest>,
) -> Result<Json<ItemLocationResponse>, ApiError> {
    let db = state
        .database
        .as_deref()
        .ok_or_else(|| ApiError::service_unavailable(uri.path()))?;
    let principal = authenticated_headers(db, &headers, uri.path()).await?;
    authorized_item(
        db,
        &principal,
        item_id,
        Permission::CollectionsWrite,
        uri.path(),
    )
    .await?;
    let expected = positive_revision(&payload.expected_revision, uri.path())?;
    let revision = db
        .move_item_to_location(item_id, payload.location_id, expected, principal.subject)
        .await
        .map_err(|error| location_error(&error, uri.path()))?;
    let location = db
        .current_item_location(item_id)
        .await
        .map_err(|error| location_error(&error, uri.path()))?;
    Ok(Json(ItemLocationResponse {
        revision: revision.to_string(),
        location: location.map(Into::into),
    }))
}

#[utoipa::path(get, path = "/api/v1/items/{item_id}/location-events", tag = "collection",
    params(("item_id" = Uuid, Path, description = "Objet")),
    responses((status = 200, body = ItemLocationEventListResponse), (status = 401, body = ProblemDetails),
        (status = 403, body = ProblemDetails), (status = 404, body = ProblemDetails)))]
async fn list_item_location_events(
    State(state): State<AppState>,
    uri: Uri,
    Path(item_id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<ItemLocationEventListResponse>, ApiError> {
    let db = state
        .database
        .as_deref()
        .ok_or_else(|| ApiError::service_unavailable(uri.path()))?;
    let principal = authenticated_headers(db, &headers, uri.path()).await?;
    let item = authorized_item(
        db,
        &principal,
        item_id,
        Permission::CollectionsRead,
        uri.path(),
    )
    .await?;
    let events = db
        .item_location_events(item_id, item.space_id)
        .await
        .map_err(|error| location_error(&error, uri.path()))?;
    Ok(Json(ItemLocationEventListResponse {
        events: events.into_iter().map(Into::into).collect(),
    }))
}

fn positive_revision(value: &str, instance: &str) -> Result<u64, ApiError> {
    value
        .parse::<u64>()
        .ok()
        .filter(|revision| *revision > 0)
        .ok_or_else(|| {
            ApiError::invalid_request(instance, "Revision attendue invalide.", "invalid_revision")
        })
}

async fn authorized_item(
    db: &Database,
    principal: &Principal,
    item_id: Uuid,
    permission: Permission,
    instance: &str,
) -> Result<Item, ApiError> {
    let item = db
        .item(item_id)
        .await
        .map_err(|error| match error.inventory_error() {
            Some(InventoryError::ItemNotFound) => ApiError::resource_not_found(instance),
            _ => ApiError::internal(instance),
        })?;
    authorized_space(db, principal, item.space_id, permission, instance).await?;
    Ok(item)
}

#[utoipa::path(get, path = "/api/v1/items/{item_id}/categories", tag = "collection",
    params(("item_id" = Uuid, Path, description = "Objet")),
    responses((status = 200, body = ItemCategoryListResponse), (status = 401, body = ProblemDetails),
        (status = 403, body = ProblemDetails), (status = 404, body = ProblemDetails)))]
async fn list_item_categories(
    State(state): State<AppState>,
    uri: Uri,
    Path(item_id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<ItemCategoryListResponse>, ApiError> {
    let db = state
        .database
        .as_deref()
        .ok_or_else(|| ApiError::service_unavailable(uri.path()))?;
    let principal = authenticated_headers(db, &headers, uri.path()).await?;
    let item = authorized_item(
        db,
        &principal,
        item_id,
        Permission::CollectionsRead,
        uri.path(),
    )
    .await?;
    let categories = db
        .item_categories(item_id, item.space_id)
        .await
        .map_err(|_| ApiError::internal(uri.path()))?;
    Ok(Json(ItemCategoryListResponse {
        revision: item.revision.to_string(),
        categories: categories.into_iter().map(Into::into).collect(),
    }))
}

#[utoipa::path(post, path = "/api/v1/items/{item_id}/categories", tag = "collection", request_body = AddItemCategoriesRequest,
    params(("item_id" = Uuid, Path, description = "Objet")),
    responses((status = 200, body = ItemCategoryListResponse), (status = 401, body = ProblemDetails),
        (status = 403, body = ProblemDetails), (status = 404, body = ProblemDetails),
        (status = 409, body = ProblemDetails), (status = 422, body = ProblemDetails)))]
async fn add_item_categories(
    State(state): State<AppState>,
    uri: Uri,
    Path(item_id): Path<Uuid>,
    headers: HeaderMap,
    Json(payload): Json<AddItemCategoriesRequest>,
) -> Result<Json<ItemCategoryListResponse>, ApiError> {
    let db = state
        .database
        .as_deref()
        .ok_or_else(|| ApiError::service_unavailable(uri.path()))?;
    let principal = authenticated_headers(db, &headers, uri.path()).await?;
    let item = authorized_item(
        db,
        &principal,
        item_id,
        Permission::CollectionsWrite,
        uri.path(),
    )
    .await?;
    let expected = positive_revision(&payload.expected_revision, uri.path())?;
    let revision = db
        .add_item_categories(item_id, expected, &payload.category_ids)
        .await
        .map_err(|error| taxonomy_error(&error, uri.path()))?;
    let categories = db
        .item_categories(item_id, item.space_id)
        .await
        .map_err(|_| ApiError::internal(uri.path()))?;
    Ok(Json(ItemCategoryListResponse {
        revision: revision.to_string(),
        categories: categories.into_iter().map(Into::into).collect(),
    }))
}

#[utoipa::path(put, path = "/api/v1/items/{item_id}/categories", tag = "collection", request_body = AddItemCategoriesRequest,
    params(("item_id" = Uuid, Path, description = "Objet")),
    responses((status = 200, body = ItemCategoryListResponse), (status = 401, body = ProblemDetails),
        (status = 403, body = ProblemDetails), (status = 404, body = ProblemDetails),
        (status = 409, body = ProblemDetails), (status = 422, body = ProblemDetails)))]
async fn replace_item_categories(
    State(state): State<AppState>,
    uri: Uri,
    Path(item_id): Path<Uuid>,
    headers: HeaderMap,
    Json(payload): Json<AddItemCategoriesRequest>,
) -> Result<Json<ItemCategoryListResponse>, ApiError> {
    let db = state
        .database
        .as_deref()
        .ok_or_else(|| ApiError::service_unavailable(uri.path()))?;
    let principal = authenticated_headers(db, &headers, uri.path()).await?;
    let item = authorized_item(
        db,
        &principal,
        item_id,
        Permission::CollectionsWrite,
        uri.path(),
    )
    .await?;
    let expected = positive_revision(&payload.expected_revision, uri.path())?;
    let revision = db
        .replace_item_categories(item_id, expected, &payload.category_ids)
        .await
        .map_err(|error| taxonomy_error(&error, uri.path()))?;
    let categories = db
        .item_categories(item_id, item.space_id)
        .await
        .map_err(|_| ApiError::internal(uri.path()))?;
    Ok(Json(ItemCategoryListResponse {
        revision: revision.to_string(),
        categories: categories.into_iter().map(Into::into).collect(),
    }))
}

#[utoipa::path(get, path = "/api/v1/items/{item_id}/fields", tag = "collection",
    params(("item_id" = Uuid, Path, description = "Objet")),
    responses((status = 200, body = EffectiveFieldListResponse), (status = 401, body = ProblemDetails),
        (status = 403, body = ProblemDetails), (status = 404, body = ProblemDetails)))]
async fn list_item_fields(
    State(state): State<AppState>,
    uri: Uri,
    Path(item_id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<EffectiveFieldListResponse>, ApiError> {
    let db = state
        .database
        .as_deref()
        .ok_or_else(|| ApiError::service_unavailable(uri.path()))?;
    let principal = authenticated_headers(db, &headers, uri.path()).await?;
    let item = authorized_item(
        db,
        &principal,
        item_id,
        Permission::CollectionsRead,
        uri.path(),
    )
    .await?;
    let fields = db
        .effective_item_fields(item_id, item.space_id)
        .await
        .map_err(|_| ApiError::internal(uri.path()))?;
    Ok(Json(EffectiveFieldListResponse {
        revision: item.revision.to_string(),
        fields: fields.into_iter().map(Into::into).collect(),
    }))
}

#[utoipa::path(put, path = "/api/v1/items/{item_id}/fields/{field_id}", tag = "collection", request_body = SetItemFieldValueRequest,
    params(("item_id" = Uuid, Path, description = "Objet"), ("field_id" = Uuid, Path, description = "Champ")),
    responses((status = 200, body = EffectiveFieldListResponse), (status = 401, body = ProblemDetails),
        (status = 403, body = ProblemDetails), (status = 404, body = ProblemDetails),
        (status = 409, body = ProblemDetails), (status = 422, body = ProblemDetails)))]
async fn set_item_field_value(
    State(state): State<AppState>,
    uri: Uri,
    Path((item_id, field_id)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
    Json(payload): Json<SetItemFieldValueRequest>,
) -> Result<Json<EffectiveFieldListResponse>, ApiError> {
    let db = state
        .database
        .as_deref()
        .ok_or_else(|| ApiError::service_unavailable(uri.path()))?;
    let principal = authenticated_headers(db, &headers, uri.path()).await?;
    let item = authorized_item(
        db,
        &principal,
        item_id,
        Permission::CollectionsWrite,
        uri.path(),
    )
    .await?;
    let expected = positive_revision(&payload.expected_revision, uri.path())?;
    let revision = db
        .set_item_field_value(item_id, field_id, expected, &payload.value)
        .await
        .map_err(|error| taxonomy_error(&error, uri.path()))?;
    let fields = db
        .effective_item_fields(item_id, item.space_id)
        .await
        .map_err(|_| ApiError::internal(uri.path()))?;
    Ok(Json(EffectiveFieldListResponse {
        revision: revision.to_string(),
        fields: fields.into_iter().map(Into::into).collect(),
    }))
}

#[utoipa::path(get, path = "/api/v1/spaces/{space_id}/items", tag = "collection",
    params(("space_id" = Uuid, Path, description = "Espace de collection"),
        ("q" = Option<String>, Query, description = "Recherche dans le nom"),
        ("after" = Option<String>, Query, description = "Dernier numero de la page precedente"),
        ("limit" = Option<u32>, Query, description = "Taille de page, de 1 a 100"),
        ("state" = Option<String>, Query, description = "Etat: active (defaut), archived, trashed ou all"),
        ("category_id" = Option<Uuid>, Query, description = "Categorie et sous-categories"),
        ("location_id" = Option<Uuid>, Query, description = "Emplacement et descendants"),
        ("group_id" = Option<Uuid>, Query, description = "Serie ou regroupement")),
    responses((status = 200, body = ItemListResponse), (status = 401, body = ProblemDetails),
        (status = 403, body = ProblemDetails), (status = 404, body = ProblemDetails)))]
#[allow(clippy::too_many_lines)]
async fn list_items(
    State(state): State<AppState>,
    uri: Uri,
    Path(space_id): Path<Uuid>,
    Query(query): Query<ListItemsQuery>,
    headers: HeaderMap,
) -> Result<Json<ItemListResponse>, ApiError> {
    let database = state
        .database
        .as_deref()
        .ok_or_else(|| ApiError::service_unavailable(uri.path()))?;
    let principal = authenticated_headers(database, &headers, uri.path()).await?;
    authorized_space(
        database,
        &principal,
        space_id,
        Permission::CollectionsRead,
        uri.path(),
    )
    .await?;
    let limit = query
        .limit
        .as_deref()
        .unwrap_or("50")
        .parse::<u32>()
        .ok()
        .filter(|value| (1..=100).contains(value))
        .ok_or_else(|| {
            ApiError::invalid_request(
                uri.path(),
                "La taille de page doit etre entre 1 et 100.",
                "invalid_limit",
            )
        })?;
    let after = query
        .after
        .as_deref()
        .unwrap_or("0")
        .parse::<u64>()
        .map_err(|_| {
            ApiError::invalid_request(uri.path(), "Curseur invalide.", "invalid_cursor")
        })?;
    let search = query.q.as_deref().unwrap_or("").trim();
    if search.chars().count() > 100 {
        return Err(ApiError::invalid_request(
            uri.path(),
            "La recherche est trop longue.",
            "invalid_search",
        ));
    }
    let state = query.state.as_deref().unwrap_or("active");
    if !matches!(state, "active" | "archived" | "trashed" | "all") {
        return Err(ApiError::invalid_request(
            uri.path(),
            "Etat d'objet invalide.",
            "invalid_state",
        ));
    }
    let category_id = query
        .category_id
        .as_deref()
        .map(Uuid::parse_str)
        .transpose()
        .map_err(|_| {
            ApiError::invalid_request(uri.path(), "Categorie invalide.", "invalid_category")
        })?;
    if let Some(category_id) = category_id {
        database
            .category(space_id, category_id)
            .await
            .map_err(|error| taxonomy_error(&error, uri.path()))?;
    }
    let location_id = query
        .location_id
        .as_deref()
        .map(Uuid::parse_str)
        .transpose()
        .map_err(|_| {
            ApiError::invalid_request(uri.path(), "Emplacement invalide.", "invalid_location")
        })?;
    if let Some(location_id) = location_id {
        database
            .location(space_id, location_id)
            .await
            .map_err(|error| location_error(&error, uri.path()))?;
    }
    let group_id = query
        .group_id
        .as_deref()
        .map(Uuid::parse_str)
        .transpose()
        .map_err(|_| {
            ApiError::invalid_request(uri.path(), "Regroupement invalide.", "invalid_group")
        })?;
    if let Some(group_id) = group_id {
        database
            .group(space_id, group_id)
            .await
            .map_err(|error| group_error(&error, uri.path()))?;
    }
    let mut items = database
        .search_items_in_space(
            space_id,
            search,
            after,
            state,
            limit + 1,
            ItemSearchFilters {
                category_id,
                location_id,
                group_id,
            },
        )
        .await
        .map_err(|_| ApiError::internal(uri.path()))?;
    let has_more = items.len() > limit as usize;
    items.truncate(limit as usize);
    let next_cursor = if has_more {
        items.last().map(|item| item.inventory_number.to_string())
    } else {
        None
    };
    Ok(Json(ItemListResponse {
        items: items.into_iter().map(Into::into).collect(),
        next_cursor,
    }))
}

#[utoipa::path(post, path = "/api/v1/spaces/{space_id}/items", tag = "collection", request_body = CreateItemRequest,
    params(("space_id" = Uuid, Path, description = "Espace de collection")),
    responses((status = 201, body = ItemResponse), (status = 401, body = ProblemDetails),
        (status = 403, body = ProblemDetails), (status = 404, body = ProblemDetails), (status = 422, body = ProblemDetails)))]
async fn create_item(
    State(state): State<AppState>,
    uri: Uri,
    Path(space_id): Path<Uuid>,
    headers: HeaderMap,
    Json(payload): Json<CreateItemRequest>,
) -> Result<Response, ApiError> {
    let database = state
        .database
        .as_deref()
        .ok_or_else(|| ApiError::service_unavailable(uri.path()))?;
    let principal = authenticated_headers(database, &headers, uri.path()).await?;
    authorized_space(
        database,
        &principal,
        space_id,
        Permission::CollectionsWrite,
        uri.path(),
    )
    .await?;
    let item = database
        .create_item(space_id, &payload.name, principal.subject)
        .await
        .map_err(|error| match error.inventory_error() {
            Some(InventoryError::InvalidName) => {
                ApiError::invalid_request(uri.path(), "Nom d'objet invalide.", "invalid_name")
            }
            _ => ApiError::internal(uri.path()),
        })?;
    let item: ItemResponse = item.into();
    let location = format!("{API_PREFIX}/items/{}", item.id);
    let mut response = Json(item).into_response();
    *response.status_mut() = StatusCode::CREATED;
    response.headers_mut().insert(
        header::LOCATION,
        HeaderValue::from_str(&location).map_err(|_| ApiError::internal(uri.path()))?,
    );
    Ok(response)
}

#[utoipa::path(get, path = "/api/v1/items/{item_id}", tag = "collection",
    params(("item_id" = Uuid, Path, description = "Objet")),
    responses((status = 200, body = ItemResponse), (status = 401, body = ProblemDetails),
        (status = 403, body = ProblemDetails), (status = 404, body = ProblemDetails)))]
async fn get_item(
    State(state): State<AppState>,
    uri: Uri,
    Path(item_id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<ItemResponse>, ApiError> {
    let database = state
        .database
        .as_deref()
        .ok_or_else(|| ApiError::service_unavailable(uri.path()))?;
    let principal = authenticated_headers(database, &headers, uri.path()).await?;
    let item = database
        .item(item_id)
        .await
        .map_err(|error| match error.inventory_error() {
            Some(InventoryError::ItemNotFound) => ApiError::resource_not_found(uri.path()),
            _ => ApiError::internal(uri.path()),
        })?;
    authorized_space(
        database,
        &principal,
        item.space_id,
        Permission::CollectionsRead,
        uri.path(),
    )
    .await?;
    Ok(Json(item.into()))
}

#[utoipa::path(patch, path = "/api/v1/items/{item_id}", tag = "collection", request_body = RenameItemRequest,
    params(("item_id" = Uuid, Path, description = "Objet a renommer")),
    responses((status = 200, body = ItemResponse), (status = 401, body = ProblemDetails),
        (status = 403, body = ProblemDetails), (status = 404, body = ProblemDetails),
        (status = 409, body = ProblemDetails), (status = 422, body = ProblemDetails)))]
async fn rename_item(
    State(state): State<AppState>,
    uri: Uri,
    Path(item_id): Path<Uuid>,
    headers: HeaderMap,
    Json(payload): Json<RenameItemRequest>,
) -> Result<Json<ItemResponse>, ApiError> {
    let database = state
        .database
        .as_deref()
        .ok_or_else(|| ApiError::service_unavailable(uri.path()))?;
    let principal = authenticated_headers(database, &headers, uri.path()).await?;
    let expected_revision = payload
        .expected_revision
        .parse::<u64>()
        .ok()
        .filter(|revision| *revision > 0)
        .ok_or_else(|| {
            ApiError::invalid_request(
                uri.path(),
                "La revision attendue doit etre un entier positif.",
                "invalid_revision",
            )
        })?;
    let item = database
        .item(item_id)
        .await
        .map_err(|error| match error.inventory_error() {
            Some(InventoryError::ItemNotFound) => ApiError::resource_not_found(uri.path()),
            _ => ApiError::internal(uri.path()),
        })?;
    authorized_space(
        database,
        &principal,
        item.space_id,
        Permission::CollectionsWrite,
        uri.path(),
    )
    .await?;
    let item = database
        .rename_item(item_id, &payload.name, expected_revision)
        .await
        .map_err(|error| match error.inventory_error() {
            Some(InventoryError::InvalidName) => {
                ApiError::invalid_request(uri.path(), "Nom d'objet invalide.", "invalid_name")
            }
            Some(InventoryError::RevisionConflict) => ApiError::revision_conflict(uri.path()),
            Some(InventoryError::ItemNotFound) => ApiError::resource_not_found(uri.path()),
            Some(InventoryError::InvalidState) => ApiError::invalid_request(
                uri.path(),
                "Cet objet est dans la corbeille ; restaurez-le avant de le modifier.",
                "invalid_state",
            ),
            _ => ApiError::internal(uri.path()),
        })?;
    Ok(Json(item.into()))
}

#[utoipa::path(put, path = "/api/v1/items/{item_id}/details", tag = "collection",
    request_body = UpdateItemDetailsRequest,
    params(("item_id" = Uuid, Path, description = "Objet")),
    responses((status = 200, body = ItemResponse), (status = 401, body = ProblemDetails),
        (status = 403, body = ProblemDetails), (status = 404, body = ProblemDetails),
        (status = 409, body = ProblemDetails), (status = 422, body = ProblemDetails)))]
async fn update_item_details(
    State(state): State<AppState>,
    uri: Uri,
    Path(item_id): Path<Uuid>,
    headers: HeaderMap,
    Json(payload): Json<UpdateItemDetailsRequest>,
) -> Result<Json<ItemResponse>, ApiError> {
    let database = state
        .database
        .as_deref()
        .ok_or_else(|| ApiError::service_unavailable(uri.path()))?;
    let principal = authenticated_headers(database, &headers, uri.path()).await?;
    let expected_revision = payload
        .expected_revision
        .parse::<u64>()
        .ok()
        .filter(|revision| *revision > 0)
        .ok_or_else(|| {
            ApiError::invalid_request(uri.path(), "Revision invalide.", "invalid_revision")
        })?;
    let item = database
        .item(item_id)
        .await
        .map_err(|error| match error.inventory_error() {
            Some(InventoryError::ItemNotFound) => ApiError::resource_not_found(uri.path()),
            _ => ApiError::internal(uri.path()),
        })?;
    authorized_space(
        database,
        &principal,
        item.space_id,
        Permission::CollectionsWrite,
        uri.path(),
    )
    .await?;
    let updated = database
        .update_item_details(
            item_id,
            payload.description.as_deref(),
            payload.historical_reference.as_deref(),
            payload.technical_reference.as_deref(),
            expected_revision,
        )
        .await
        .map_err(|error| match error.inventory_error() {
            Some(InventoryError::InvalidDetails) => ApiError::invalid_request(
                uri.path(),
                "Description ou reference trop longue.",
                "invalid_details",
            ),
            Some(InventoryError::RevisionConflict) => ApiError::revision_conflict(uri.path()),
            Some(InventoryError::ItemNotFound) => ApiError::resource_not_found(uri.path()),
            Some(InventoryError::InvalidState) => ApiError::invalid_request(
                uri.path(),
                "Restaurez l'objet avant de le modifier.",
                "invalid_state",
            ),
            _ => ApiError::internal(uri.path()),
        })?;
    Ok(Json(updated.into()))
}

/// Parses the expected revision, loads the item, checks space write access and applies a state.
async fn transition_item(
    state: &AppState,
    item_id: Uuid,
    payload: ItemStateRequest,
    headers: &HeaderMap,
    uri: &Uri,
    target: ItemState,
) -> Result<Json<ItemResponse>, ApiError> {
    let database = state
        .database
        .as_deref()
        .ok_or_else(|| ApiError::service_unavailable(uri.path()))?;
    let principal = authenticated_headers(database, headers, uri.path()).await?;
    let expected_revision = payload
        .expected_revision
        .parse::<u64>()
        .ok()
        .filter(|revision| *revision > 0)
        .ok_or_else(|| {
            ApiError::invalid_request(
                uri.path(),
                "La revision attendue doit etre un entier positif.",
                "invalid_revision",
            )
        })?;
    let item = database
        .item(item_id)
        .await
        .map_err(|error| match error.inventory_error() {
            Some(InventoryError::ItemNotFound) => ApiError::resource_not_found(uri.path()),
            _ => ApiError::internal(uri.path()),
        })?;
    authorized_space(
        database,
        &principal,
        item.space_id,
        Permission::CollectionsWrite,
        uri.path(),
    )
    .await?;
    let item = database
        .set_item_state(item_id, target, expected_revision, principal.subject)
        .await
        .map_err(|error| match error.inventory_error() {
            Some(InventoryError::RevisionConflict) => ApiError::revision_conflict(uri.path()),
            Some(InventoryError::InvalidTransition) => ApiError::invalid_request(
                uri.path(),
                "Cette transition d'etat n'est pas autorisee.",
                "invalid_transition",
            ),
            Some(InventoryError::ItemNotFound) => ApiError::resource_not_found(uri.path()),
            _ => ApiError::internal(uri.path()),
        })?;
    Ok(Json(item.into()))
}

#[utoipa::path(post, path = "/api/v1/items/{item_id}/archive", tag = "collection", request_body = ItemStateRequest,
    params(("item_id" = Uuid, Path, description = "Objet a archiver")),
    responses((status = 200, body = ItemResponse), (status = 401, body = ProblemDetails),
        (status = 403, body = ProblemDetails), (status = 404, body = ProblemDetails),
        (status = 409, body = ProblemDetails), (status = 422, body = ProblemDetails)))]
async fn archive_item(
    State(state): State<AppState>,
    uri: Uri,
    Path(item_id): Path<Uuid>,
    headers: HeaderMap,
    Json(payload): Json<ItemStateRequest>,
) -> Result<Json<ItemResponse>, ApiError> {
    transition_item(
        &state,
        item_id,
        payload,
        &headers,
        &uri,
        ItemState::Archived,
    )
    .await
}

#[utoipa::path(post, path = "/api/v1/items/{item_id}/trash", tag = "collection", request_body = ItemStateRequest,
    params(("item_id" = Uuid, Path, description = "Objet a mettre a la corbeille")),
    responses((status = 200, body = ItemResponse), (status = 401, body = ProblemDetails),
        (status = 403, body = ProblemDetails), (status = 404, body = ProblemDetails),
        (status = 409, body = ProblemDetails), (status = 422, body = ProblemDetails)))]
async fn trash_item(
    State(state): State<AppState>,
    uri: Uri,
    Path(item_id): Path<Uuid>,
    headers: HeaderMap,
    Json(payload): Json<ItemStateRequest>,
) -> Result<Json<ItemResponse>, ApiError> {
    transition_item(&state, item_id, payload, &headers, &uri, ItemState::Trashed).await
}

#[utoipa::path(post, path = "/api/v1/items/{item_id}/restore", tag = "collection", request_body = ItemStateRequest,
    params(("item_id" = Uuid, Path, description = "Objet a restaurer")),
    responses((status = 200, body = ItemResponse), (status = 401, body = ProblemDetails),
        (status = 403, body = ProblemDetails), (status = 404, body = ProblemDetails),
        (status = 409, body = ProblemDetails), (status = 422, body = ProblemDetails)))]
async fn restore_item(
    State(state): State<AppState>,
    uri: Uri,
    Path(item_id): Path<Uuid>,
    headers: HeaderMap,
    Json(payload): Json<ItemStateRequest>,
) -> Result<Json<ItemResponse>, ApiError> {
    transition_item(&state, item_id, payload, &headers, &uri, ItemState::Active).await
}

#[utoipa::path(get, path = "/api/v1/items/{item_id}/state-events", tag = "collection",
    params(("item_id" = Uuid, Path, description = "Objet")),
    responses((status = 200, body = ItemStateEventListResponse), (status = 401, body = ProblemDetails),
        (status = 404, body = ProblemDetails)))]
async fn list_item_state_events(
    State(state): State<AppState>,
    uri: Uri,
    Path(item_id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<ItemStateEventListResponse>, ApiError> {
    let database = state
        .database
        .as_deref()
        .ok_or_else(|| ApiError::service_unavailable(uri.path()))?;
    let principal = authenticated_headers(database, &headers, uri.path()).await?;
    let item = database
        .item(item_id)
        .await
        .map_err(|error| match error.inventory_error() {
            Some(InventoryError::ItemNotFound) => ApiError::resource_not_found(uri.path()),
            _ => ApiError::internal(uri.path()),
        })?;
    let space = database
        .space(item.space_id)
        .await
        .map_err(|error| match error.space_error() {
            Some(SpaceError::SpaceNotFound) => ApiError::resource_not_found(uri.path()),
            _ => ApiError::internal(uri.path()),
        })?;
    if !can_manage_members(&principal, space.owner_account_id) {
        return Err(ApiError::resource_not_found(uri.path()));
    }
    let events = database
        .item_state_events(item_id, item.space_id)
        .await
        .map_err(|_| ApiError::internal(uri.path()))?;
    Ok(Json(ItemStateEventListResponse {
        events: events.into_iter().map(Into::into).collect(),
    }))
}

#[utoipa::path(post, path = "/api/v1/items/{item_id}/transfers", tag = "collection", request_body = TransferItemRequest,
    params(("item_id" = Uuid, Path, description = "Objet a transferer")),
    responses((status = 200, body = TransferOutcomeResponse), (status = 401, body = ProblemDetails),
        (status = 403, body = ProblemDetails), (status = 404, body = ProblemDetails),
        (status = 409, body = ProblemDetails), (status = 422, body = ProblemDetails)))]
async fn transfer_item(
    State(state): State<AppState>,
    uri: Uri,
    Path(item_id): Path<Uuid>,
    headers: HeaderMap,
    Json(payload): Json<TransferItemRequest>,
) -> Result<Json<TransferOutcomeResponse>, ApiError> {
    let database = state
        .database
        .as_deref()
        .ok_or_else(|| ApiError::service_unavailable(uri.path()))?;
    let principal = authenticated_headers(database, &headers, uri.path()).await?;
    let expected_revision = payload
        .expected_revision
        .parse::<u64>()
        .ok()
        .filter(|revision| *revision > 0)
        .ok_or_else(|| {
            ApiError::invalid_request(
                uri.path(),
                "La revision attendue doit etre un entier positif.",
                "invalid_revision",
            )
        })?;
    let item = database
        .item(item_id)
        .await
        .map_err(|error| match error.inventory_error() {
            Some(InventoryError::ItemNotFound) => ApiError::resource_not_found(uri.path()),
            _ => ApiError::internal(uri.path()),
        })?;
    authorized_space(
        database,
        &principal,
        item.space_id,
        Permission::CollectionsWrite,
        uri.path(),
    )
    .await?;
    authorized_space(
        database,
        &principal,
        payload.destination_space_id,
        Permission::CollectionsWrite,
        uri.path(),
    )
    .await?;
    let outcome = database
        .transfer_item_with_categories(
            item_id,
            payload.destination_space_id,
            expected_revision,
            principal.subject,
            payload.destination_category_ids.as_deref().unwrap_or(&[]),
        )
        .await
        .map_err(|error| match error.inventory_error() {
            Some(InventoryError::RevisionConflict) => ApiError::revision_conflict(uri.path()),
            Some(InventoryError::SameSpace) => ApiError::invalid_request(
                uri.path(),
                "L'objet est deja dans cet espace.",
                "same_space",
            ),
            Some(InventoryError::ItemNotFound | InventoryError::SpaceNotFound) => {
                ApiError::resource_not_found(uri.path())
            }
            Some(InventoryError::InvalidState) => ApiError::invalid_request(
                uri.path(),
                "Cet objet est dans la corbeille ; restaurez-le avant de le transferer.",
                "invalid_state",
            ),
            Some(InventoryError::DestinationCategoriesRequired) => ApiError::invalid_request(
                uri.path(),
                "Choisissez au moins une categorie de destination avant le transfert.",
                "destination_categories_required",
            ),
            Some(InventoryError::InvalidDestinationCategory) => ApiError::invalid_request(
                uri.path(),
                "Une categorie choisie n'appartient pas a l'espace de destination.",
                "invalid_destination_category",
            ),
            _ => ApiError::internal(uri.path()),
        })?;
    Ok(Json(outcome.into()))
}

#[utoipa::path(get, path = "/api/v1/items/{item_id}/transfers", tag = "collection",
    params(("item_id" = Uuid, Path, description = "Objet")),
    responses((status = 200, body = ItemTransferListResponse), (status = 401, body = ProblemDetails),
        (status = 403, body = ProblemDetails), (status = 404, body = ProblemDetails)))]
async fn list_item_transfers(
    State(state): State<AppState>,
    uri: Uri,
    Path(item_id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<ItemTransferListResponse>, ApiError> {
    let database = state
        .database
        .as_deref()
        .ok_or_else(|| ApiError::service_unavailable(uri.path()))?;
    let principal = authenticated_headers(database, &headers, uri.path()).await?;
    let item = database
        .item(item_id)
        .await
        .map_err(|error| match error.inventory_error() {
            Some(InventoryError::ItemNotFound) => ApiError::resource_not_found(uri.path()),
            _ => ApiError::internal(uri.path()),
        })?;
    authorized_space(
        database,
        &principal,
        item.space_id,
        Permission::CollectionsRead,
        uri.path(),
    )
    .await?;
    let transfers = database
        .item_transfers(item_id)
        .await
        .map_err(|_| ApiError::internal(uri.path()))?;
    for transfer in &transfers {
        for space_id in [transfer.source_space_id, transfer.destination_space_id] {
            if let Err(error) = authorized_space(
                database,
                &principal,
                space_id,
                Permission::CollectionsRead,
                uri.path(),
            )
            .await
            {
                if error.status == StatusCode::INTERNAL_SERVER_ERROR {
                    return Err(error);
                }
                return Err(ApiError::resource_not_found(uri.path()));
            }
        }
    }
    Ok(Json(ItemTransferListResponse {
        transfers: transfers.into_iter().map(Into::into).collect(),
    }))
}

fn session_error_to_api(instance: &str, error: &DatabaseError) -> ApiError {
    match error.session_error() {
        Some(SessionError::Unknown | SessionError::Expired) => {
            ApiError::authentication_required(instance)
        }
        _ => ApiError::internal(instance),
    }
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
    paths(
        service_info,
        health,
        liveness,
        readiness,
        session,
        create_session,
        list_sessions,
        revoke_session,
        list_spaces,
        my_space_permissions,
        list_wishes,
        create_wish,
        update_wish,
        list_vendors,
        create_vendor,
        update_vendor,
        list_offers,
        create_offer,
        update_offer,
        list_admin_spaces,
        create_space,
        list_categories,
        create_category,
        rename_category,
        list_category_fields,
        create_category_field,
        rename_category_field,
        list_locations,
        create_location,
        list_groups,
        create_group,
        rename_group,
        delete_group,
        list_item_groups,
        replace_item_groups,
        list_item_relations,
        replace_item_relations,
        get_item_location,
        move_item_location,
        list_item_location_events,
        list_item_categories,
        add_item_categories,
        replace_item_categories,
        list_item_fields,
        set_item_field_value,
        list_items,
        create_item,
        get_item,
        rename_item,
        update_item_details,
        archive_item,
        trash_item,
        restore_item,
        list_item_state_events,
        transfer_item,
        list_item_transfers,
        create_invitation,
        list_invitations,
        revoke_invitation,
        accept_invitation,
        list_members,
        update_member_permissions,
        remove_member,
        transfer_space_ownership
    ),
    components(schemas(
        HealthResponse,
        ServiceInfoResponse,
        ProblemDetails,
        SessionResponse,
        SessionSummary,
        SessionListResponse,
        LoginRequest,
        LoginResponse,
        SpaceResponse, SpaceListResponse, SpacePermissionsResponse, CreateSpaceRequest,
        WishResponse, WishListResponse, CreateWishRequest, UpdateWishRequest,
        VendorResponse, VendorListResponse, CreateVendorRequest, UpdateVendorRequest,
        OfferResponse, OfferListResponse, CreateOfferRequest, UpdateOfferRequest,
        CategoryResponse, CategoryListResponse, CreateCategoryRequest,
        CategoryFieldResponse, CategoryFieldListResponse, CreateCategoryFieldRequest, RenameDefinitionRequest,
        LocationResponse, LocationListResponse, CreateLocationRequest,
        GroupResponse, GroupListResponse, CreateGroupRequest, RenameGroupRequest, DeleteGroupRequest,
        ItemGroupListResponse, ReplaceItemGroupsRequest,
        ItemRelationResponse, ItemRelationListResponse, ItemRelationInput, ReplaceItemRelationsRequest,
        ItemLocationResponse, MoveItemLocationRequest,
        ItemLocationEventResponse, ItemLocationEventListResponse,
        ItemCategoryResponse, ItemCategoryListResponse, AddItemCategoriesRequest,
        EffectiveFieldResponse, EffectiveFieldListResponse, SetItemFieldValueRequest,
        CreateInvitationRequest, InvitationResponse, InvitationListResponse, AcceptInvitationRequest,
        SpaceMemberResponse, SpaceMemberListResponse, UpdateMemberPermissionsRequest,
        TransferSpaceOwnershipRequest, SpaceOwnershipResponse,
        ItemResponse, ItemListResponse, CreateItemRequest, RenameItemRequest, UpdateItemDetailsRequest, ItemStateRequest,
        TransferItemRequest, ItemTransferResponse, TransferOutcomeResponse, ItemTransferListResponse,
        ItemStateEventResponse, ItemStateEventListResponse,
        IdleTimeout,
        Principal,
        crate::security::Permission
    )),
    tags(
        (name = "system", description = "État et métadonnées du service"),
        (name = "authentication", description = "Session et identité applicative"),
        (name = "collection", description = "Espaces et inventaire"),
        (name = "acquisitions", description = "Envies, fournisseurs et offres sans montants"),
        (name = "sharing", description = "Invitations et partage des espaces")
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

/// Builds the `CollectionOps` HTTP router without persistence.
///
/// Useful for tests and for a deployment that only serves metadata. The login routes are
/// absent, so they answer `404` rather than failing later on a missing pool.
pub fn app() -> Router {
    app_with_state(AppState {
        database: None,
        passwords: Arc::new(PasswordService::default()),
        mail: None,
    })
}

/// Builds the `CollectionOps` HTTP router with persistence enabled.
pub fn app_with_database(database: Database) -> Router {
    app_with_database_and_mail(database, None)
}

/// Builds the persistent router with optional SMTP delivery for invitations.
pub fn app_with_database_and_mail(database: Database, mail: Option<SmtpDelivery>) -> Router {
    app_with_state(AppState {
        database: Some(Arc::new(database)),
        passwords: Arc::new(PasswordService::default()),
        mail: mail.map(Arc::new),
    })
}

fn taxonomy_routes() -> Router<AppState> {
    Router::new()
        .route(
            &format!("{API_PREFIX}/spaces/{{space_id}}/categories"),
            get(list_categories).post(create_category),
        )
        .route(
            &format!("{API_PREFIX}/spaces/{{space_id}}/categories/{{category_id}}"),
            axum::routing::patch(rename_category),
        )
        .route(
            &format!("{API_PREFIX}/spaces/{{space_id}}/categories/{{category_id}}/fields"),
            get(list_category_fields).post(create_category_field),
        )
        .route(
            &format!(
                "{API_PREFIX}/spaces/{{space_id}}/categories/{{category_id}}/fields/{{field_id}}"
            ),
            axum::routing::patch(rename_category_field),
        )
        .route(
            &format!("{API_PREFIX}/items/{{item_id}}/categories"),
            get(list_item_categories)
                .post(add_item_categories)
                .put(replace_item_categories),
        )
        .route(
            &format!("{API_PREFIX}/items/{{item_id}}/fields"),
            get(list_item_fields),
        )
        .route(
            &format!("{API_PREFIX}/items/{{item_id}}/fields/{{field_id}}"),
            put(set_item_field_value),
        )
}

fn location_routes() -> Router<AppState> {
    Router::new()
        .route(
            &format!("{API_PREFIX}/spaces/{{space_id}}/locations"),
            get(list_locations).post(create_location),
        )
        .route(
            &format!("{API_PREFIX}/items/{{item_id}}/location"),
            get(get_item_location).put(move_item_location),
        )
        .route(
            &format!("{API_PREFIX}/items/{{item_id}}/location-events"),
            get(list_item_location_events),
        )
}

fn group_routes() -> Router<AppState> {
    Router::new()
        .route(
            &format!("{API_PREFIX}/items/{{item_id}}/details"),
            axum::routing::put(update_item_details),
        )
        .route(
            &format!("{API_PREFIX}/spaces/{{space_id}}/groups"),
            get(list_groups).post(create_group),
        )
        .route(
            &format!("{API_PREFIX}/spaces/{{space_id}}/groups/{{group_id}}"),
            axum::routing::patch(rename_group).delete(delete_group),
        )
        .route(
            &format!("{API_PREFIX}/items/{{item_id}}/groups"),
            get(list_item_groups).put(replace_item_groups),
        )
}

fn relation_routes() -> Router<AppState> {
    Router::new().route(
        &format!("{API_PREFIX}/items/{{item_id}}/relations"),
        get(list_item_relations).put(replace_item_relations),
    )
}

fn acquisition_routes() -> Router<AppState> {
    Router::new()
        .route(
            &format!("{API_PREFIX}/spaces/{{space_id}}/my-permissions"),
            get(my_space_permissions),
        )
        .route(
            &format!("{API_PREFIX}/spaces/{{space_id}}/wishes"),
            get(list_wishes).post(create_wish),
        )
        .route(
            &format!("{API_PREFIX}/spaces/{{space_id}}/wishes/{{wish_id}}"),
            axum::routing::patch(update_wish),
        )
        .route(
            &format!("{API_PREFIX}/spaces/{{space_id}}/vendors"),
            get(list_vendors).post(create_vendor),
        )
        .route(
            &format!("{API_PREFIX}/spaces/{{space_id}}/vendors/{{vendor_id}}"),
            axum::routing::patch(update_vendor),
        )
        .route(
            &format!("{API_PREFIX}/spaces/{{space_id}}/wishes/{{wish_id}}/offers"),
            get(list_offers).post(create_offer),
        )
        .route(
            &format!("{API_PREFIX}/spaces/{{space_id}}/wishes/{{wish_id}}/offers/{{offer_id}}"),
            axum::routing::patch(update_offer),
        )
}

#[allow(clippy::too_many_lines)]
fn app_with_state(state: AppState) -> Router {
    let authentication = if state.database.is_some() {
        // Login is only reachable when a pool exists, so no handler has to cope with `None`.
        Router::new()
            .route(&format!("{API_PREFIX}/sessions"), post(create_session))
            .route(&format!("{API_PREFIX}/sessions"), get(list_sessions))
            .route(
                &format!("{API_PREFIX}/sessions/{{session_id}}"),
                delete(revoke_session),
            )
            .route(
                &format!("{API_PREFIX}/spaces"),
                get(list_spaces).post(create_space),
            )
            .merge(taxonomy_routes())
            .merge(location_routes())
            .merge(group_routes())
            .merge(relation_routes())
            .merge(acquisition_routes())
            .route(
                &format!("{API_PREFIX}/admin/spaces"),
                get(list_admin_spaces),
            )
            .route(
                &format!("{API_PREFIX}/spaces/{{space_id}}/items"),
                get(list_items).post(create_item),
            )
            .route(
                &format!("{API_PREFIX}/spaces/{{space_id}}/invitations"),
                get(list_invitations).post(create_invitation),
            )
            .route(
                &format!("{API_PREFIX}/spaces/{{space_id}}/members"),
                get(list_members),
            )
            .route(
                &format!("{API_PREFIX}/spaces/{{space_id}}/members/{{account_id}}/permissions"),
                put(update_member_permissions),
            )
            .route(
                &format!("{API_PREFIX}/spaces/{{space_id}}/members/{{account_id}}"),
                delete(remove_member),
            )
            .route(
                &format!("{API_PREFIX}/spaces/{{space_id}}/ownership"),
                post(transfer_space_ownership),
            )
            .route(
                &format!("{API_PREFIX}/spaces/{{space_id}}/invitations/{{invitation_id}}"),
                delete(revoke_invitation),
            )
            .route(
                &format!("{API_PREFIX}/invitations/{{invitation_id}}/accept"),
                post(accept_invitation),
            )
            .route(
                &format!("{API_PREFIX}/items/{{item_id}}"),
                get(get_item).patch(rename_item),
            )
            .route(
                &format!("{API_PREFIX}/items/{{item_id}}/archive"),
                post(archive_item),
            )
            .route(
                &format!("{API_PREFIX}/items/{{item_id}}/trash"),
                post(trash_item),
            )
            .route(
                &format!("{API_PREFIX}/items/{{item_id}}/restore"),
                post(restore_item),
            )
            .route(
                &format!("{API_PREFIX}/items/{{item_id}}/state-events"),
                get(list_item_state_events),
            )
            .route(
                &format!("{API_PREFIX}/items/{{item_id}}/transfers"),
                get(list_item_transfers).post(transfer_item),
            )
    } else {
        Router::new()
    };

    Router::new()
        .route(API_PREFIX, get(service_info))
        .route(&format!("{API_PREFIX}/health"), get(health))
        .route(&format!("{API_PREFIX}/health/live"), get(liveness))
        .route(&format!("{API_PREFIX}/health/ready"), get(readiness))
        .route(&format!("{API_PREFIX}/session"), get(session))
        .merge(authentication)
        .route("/api/openapi.json", get(openapi))
        .with_state(state)
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
