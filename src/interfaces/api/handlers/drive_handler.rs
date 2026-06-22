//! Drive endpoints.
//!
//! - `GET    /api/drives`                              — list every drive the caller can read (D0)
//! - `GET    /api/drives/{id}/members`                 — list role grants on a drive (D2)
//! - `POST   /api/drives/{id}/members`                 — add a member (D2)
//! - `PATCH  /api/drives/{id}/members/{kind}/{sid}`    — change a member's role / expiry (D2)
//! - `DELETE /api/drives/{id}/members/{kind}/{sid}`    — remove a member (D2)
//!
//! D3 adds the create-shared-drive flow under `POST /api/drives`. The
//! membership endpoints are thin wrappers around `DriveManagementService`,
//! which layers the personal-drive guard and shared-drive last-owner
//! protection on top of the generic `role_grants` write path.

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::Deserialize;
use tracing::error;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::application::dtos::drive_dto::DriveDto;
use crate::application::dtos::grant_dto::{GrantDto, RoleDto, SubjectDto, SubjectTypeDto};
use crate::common::di::AppState;
use crate::domain::repositories::drive_repository::DriveRepository;
use crate::domain::services::authorization::Subject;
use crate::interfaces::errors::AppError;
use crate::interfaces::middleware::auth::AuthUser;

#[utoipa::path(
    get,
    path = "/api/drives",
    responses(
        (status = 200, description = "Drives the caller can read", body = Vec<DriveDto>),
        (status = 500, description = "Internal server error"),
    ),
    security(("bearerAuth" = [])),
    tag = "drives"
)]
pub async fn list_drives(
    State(state): State<Arc<AppState>>,
    auth_user: AuthUser,
) -> impl IntoResponse {
    let caller_id = auth_user.id;

    let (subject_types, subject_ids) = match state
        .authorization
        .expand_subject_for_listing(Subject::User(caller_id))
        .await
    {
        Ok(pair) => pair,
        Err(e) => {
            error!("list_drives: subject expansion failed: {e}");
            return AppError::from(e).into_response();
        }
    };

    match state
        .drive_repo
        .list_for_subjects(&subject_types, &subject_ids)
        .await
    {
        Ok(drives) => {
            let dtos: Vec<DriveDto> = drives.into_iter().map(DriveDto::from).collect();
            (StatusCode::OK, Json(dtos)).into_response()
        }
        Err(e) => {
            error!("list_drives: repo lookup failed: {e}");
            AppError::internal_error(format!("Failed to list drives: {e}")).into_response()
        }
    }
}

// ════════════════════════════════════════════════════════════════════════════
// Membership API (D2)
// ════════════════════════════════════════════════════════════════════════════

/// Body for `POST /api/drives/{id}/members`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct AddDriveMemberDto {
    pub subject: SubjectDto,
    pub role: RoleDto,
    #[serde(default)]
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Body for `PATCH /api/drives/{id}/members/{kind}/{sid}`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateDriveMemberDto {
    pub role: RoleDto,
    /// Optional. Pass `null` (or omit) to clear an existing expiry.
    #[serde(default)]
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
}

fn parse_subject(kind: SubjectTypeDto, id: Uuid) -> Subject {
    match kind {
        SubjectTypeDto::User => Subject::User(id),
        SubjectTypeDto::Group => Subject::Group(id),
        SubjectTypeDto::Token => Subject::Token(id),
    }
}

#[utoipa::path(
    get,
    path = "/api/drives/{id}/members",
    params(("id" = Uuid, Path, description = "Drive UUID")),
    responses(
        (status = 200, description = "Role grants on this drive", body = Vec<GrantDto>),
        (status = 404, description = "Drive not found or caller lacks Read"),
    ),
    security(("bearerAuth" = [])),
    tag = "drives"
)]
pub async fn list_drive_members(
    State(state): State<Arc<AppState>>,
    auth_user: AuthUser,
    Path(drive_id): Path<Uuid>,
) -> impl IntoResponse {
    match state
        .drive_management_service
        .list_members(auth_user.id, drive_id)
        .await
    {
        Ok(grants) => {
            let dtos: Vec<GrantDto> = grants.into_iter().map(GrantDto::from).collect();
            (StatusCode::OK, Json(dtos)).into_response()
        }
        Err(e) => AppError::from(e).into_response(),
    }
}

#[utoipa::path(
    post,
    path = "/api/drives/{id}/members",
    params(("id" = Uuid, Path, description = "Drive UUID")),
    request_body = AddDriveMemberDto,
    responses(
        (status = 201, description = "Member added", body = GrantDto),
        (status = 400, description = "Validation error (e.g. last-owner constraint)"),
        (status = 404, description = "Drive not found or caller lacks Manage"),
        (status = 405, description = "Personal drive — membership is immutable"),
    ),
    security(("bearerAuth" = [])),
    tag = "drives"
)]
pub async fn add_drive_member(
    State(state): State<Arc<AppState>>,
    auth_user: AuthUser,
    Path(drive_id): Path<Uuid>,
    Json(dto): Json<AddDriveMemberDto>,
) -> impl IntoResponse {
    let subject = parse_subject(dto.subject.kind, dto.subject.id);
    match state
        .drive_management_service
        .set_member_role(
            auth_user.id,
            drive_id,
            subject,
            dto.role.into(),
            dto.expires_at,
        )
        .await
    {
        Ok(grant) => (StatusCode::CREATED, Json(GrantDto::from(grant))).into_response(),
        Err(e) => AppError::from(e).into_response(),
    }
}

#[utoipa::path(
    patch,
    path = "/api/drives/{id}/members/{kind}/{sid}",
    params(
        ("id" = Uuid, Path, description = "Drive UUID"),
        ("kind" = String, Path, description = "Subject kind: user|group|token"),
        ("sid" = Uuid, Path, description = "Subject UUID"),
    ),
    request_body = UpdateDriveMemberDto,
    responses(
        (status = 200, description = "Member role updated", body = GrantDto),
        (status = 400, description = "Validation error (e.g. last-owner demotion)"),
        (status = 404, description = "Drive not found or caller lacks Manage"),
        (status = 405, description = "Personal drive — membership is immutable"),
    ),
    security(("bearerAuth" = [])),
    tag = "drives"
)]
pub async fn update_drive_member(
    State(state): State<Arc<AppState>>,
    auth_user: AuthUser,
    Path((drive_id, kind, subject_id)): Path<(Uuid, SubjectTypeDto, Uuid)>,
    Json(dto): Json<UpdateDriveMemberDto>,
) -> impl IntoResponse {
    let subject = parse_subject(kind, subject_id);
    match state
        .drive_management_service
        .set_member_role(
            auth_user.id,
            drive_id,
            subject,
            dto.role.into(),
            dto.expires_at,
        )
        .await
    {
        Ok(grant) => (StatusCode::OK, Json(GrantDto::from(grant))).into_response(),
        Err(e) => AppError::from(e).into_response(),
    }
}

#[utoipa::path(
    delete,
    path = "/api/drives/{id}/members/{kind}/{sid}",
    params(
        ("id" = Uuid, Path, description = "Drive UUID"),
        ("kind" = String, Path, description = "Subject kind: user|group|token"),
        ("sid" = Uuid, Path, description = "Subject UUID"),
    ),
    responses(
        (status = 204, description = "Member removed (or was never a member — idempotent)"),
        (status = 400, description = "Last-owner protection — promote another member first"),
        (status = 404, description = "Drive not found or caller lacks Manage"),
        (status = 405, description = "Personal drive — membership is immutable"),
    ),
    security(("bearerAuth" = [])),
    tag = "drives"
)]
pub async fn remove_drive_member(
    State(state): State<Arc<AppState>>,
    auth_user: AuthUser,
    Path((drive_id, kind, subject_id)): Path<(Uuid, SubjectTypeDto, Uuid)>,
) -> impl IntoResponse {
    let subject = parse_subject(kind, subject_id);
    match state
        .drive_management_service
        .remove_member(auth_user.id, drive_id, subject)
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => AppError::from(e).into_response(),
    }
}
