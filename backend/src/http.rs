use std::collections::BTreeSet;
use std::sync::Arc;

use axum::{
    Extension, Json, Router,
    body::Body,
    extract::{Path, Query, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, Uri, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use serde::{Deserialize, Serialize};
use tower_http::trace::TraceLayer;
use utoipa::{OpenApi, ToSchema};
use uuid::Uuid;

use crate::{
    credentials::{EmailAddress, IdleTimeout, PasswordService, SessionToken, SessionTokenError},
    database::{
        Database, DatabaseError, InventoryError, IssuedSession, Item, ItemTransfer, SessionError,
        SessionRecord, Space, SpaceError, TransferOutcome,
    },
    security::{Permission, Principal},
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
                detail: "L'objet a change depuis sa derniere lecture. Actualisez-le avant de recommencer."
                    .to_owned(),
                instance: instance.to_owned(),
                code: "revision_conflict",
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
        principal: collection_principal(issued.account_id, issued.display_name),
    }
}

// At this stage every authenticated account may use collection features, but a space still
// requires an explicit membership and grant. Other application permissions are never inferred.
fn collection_principal(subject: Uuid, display_name: String) -> Principal {
    Principal {
        subject,
        display_name,
        roles: BTreeSet::default(),
        permissions: [Permission::CollectionsRead, Permission::CollectionsWrite].into(),
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
    Ok(collection_principal(subject, session.display_name))
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
pub struct CreateSpaceRequest {
    pub name: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ItemResponse {
    pub id: Uuid,
    pub space_id: Uuid,
    pub inventory_number: String,
    pub name: String,
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
}
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CreateItemRequest {
    pub name: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct TransferItemRequest {
    pub destination_space_id: Uuid,
    pub expected_revision: String,
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
        if principal
            .require_space_permission(
                space.id,
                membership
                    .as_ref()
                    .map(crate::database::Membership::as_space_membership)
                    .as_ref(),
                Permission::CollectionsRead,
            )
            .is_ok()
        {
            visible.push(space.into());
        }
    }
    Ok(Json(SpaceListResponse { spaces: visible }))
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

#[utoipa::path(get, path = "/api/v1/spaces/{space_id}/items", tag = "collection",
    params(("space_id" = Uuid, Path, description = "Espace de collection"),
        ("q" = Option<String>, Query, description = "Recherche dans le nom"),
        ("after" = Option<String>, Query, description = "Dernier numero de la page precedente"),
        ("limit" = Option<u32>, Query, description = "Taille de page, de 1 a 100")),
    responses((status = 200, body = ItemListResponse), (status = 401, body = ProblemDetails),
        (status = 403, body = ProblemDetails), (status = 404, body = ProblemDetails)))]
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
    let mut items = database
        .search_items_in_space(space_id, search, after, limit + 1)
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
) -> Result<(StatusCode, Json<ItemResponse>), ApiError> {
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
    Ok((StatusCode::CREATED, Json(item.into())))
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
        .transfer_item(
            item_id,
            payload.destination_space_id,
            expected_revision,
            principal.subject,
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
        create_space,
        list_items,
        create_item,
        get_item,
        transfer_item,
        list_item_transfers
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
        SpaceResponse, SpaceListResponse, CreateSpaceRequest,
        ItemResponse, ItemListResponse, CreateItemRequest,
        TransferItemRequest, ItemTransferResponse, TransferOutcomeResponse, ItemTransferListResponse,
        IdleTimeout,
        Principal,
        crate::security::Permission
    )),
    tags(
        (name = "system", description = "État et métadonnées du service"),
        (name = "authentication", description = "Session et identité applicative"),
        (name = "collection", description = "Espaces et inventaire")
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
    })
}

/// Builds the `CollectionOps` HTTP router with persistence enabled.
pub fn app_with_database(database: Database) -> Router {
    app_with_state(AppState {
        database: Some(Arc::new(database)),
        passwords: Arc::new(PasswordService::default()),
    })
}

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
            .route(
                &format!("{API_PREFIX}/spaces/{{space_id}}/items"),
                get(list_items).post(create_item),
            )
            .route(&format!("{API_PREFIX}/items/{{item_id}}"), get(get_item))
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
