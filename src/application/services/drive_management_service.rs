//! D2 — drive membership management service.
//!
//! Translates `POST/PATCH/DELETE /api/drives/{id}/members` into role-grant
//! writes on `resource_type='drive'`, layering D2's business rules on top:
//!
//! - **Personal-drive guard** (§2): drives with `kind='personal'` are
//!   single-user single-owner by invariant; any member mutation is refused
//!   at the service edge with `403`. Listing a personal drive's members is
//!   still allowed (returns exactly the owner row) so the UI can render the
//!   same shape across drive kinds without per-kind branching.
//!
//! - **Last-owner protection** (shared drives): removing or demoting the
//!   final `Role::Owner` would leave the drive unmanageable. Refused at the
//!   service edge — the caller has to promote someone else to Owner first,
//!   or delete the drive.
//!
//! - **Authorization**: caller must hold `Permission::Manage` on the drive
//!   to mutate; `Permission::Read` to list. `AuthorizationEngine::require`
//!   emits the canonical `authz.denied` audit line on rejection.

use std::sync::Arc;

use uuid::Uuid;

use crate::application::ports::authorization_ports::AuthorizationEngine;
use crate::common::errors::DomainError;
use crate::domain::repositories::drive_repository::DriveRepository;
use crate::domain::services::authorization::{Grant, Permission, Resource, Role, Subject};
use crate::infrastructure::repositories::pg::DrivePgRepository;
use crate::infrastructure::services::pg_acl_engine::PgAclEngine;

pub struct DriveManagementService {
    drive_repo: Arc<DrivePgRepository>,
    authz: Arc<PgAclEngine>,
}

impl DriveManagementService {
    pub fn new(drive_repo: Arc<DrivePgRepository>, authz: Arc<PgAclEngine>) -> Self {
        Self { drive_repo, authz }
    }

    /// `GET /api/drives/{id}/members` — every role grant on the drive.
    pub async fn list_members(
        &self,
        caller_id: Uuid,
        drive_id: Uuid,
    ) -> Result<Vec<Grant>, DomainError> {
        let resource = Resource::Drive(drive_id);
        self.authz
            .require(Subject::User(caller_id), Permission::Read, resource)
            .await?;
        self.authz.list_grants_on_resource(resource).await
    }

    /// `POST /api/drives/{id}/members` (create) or
    /// `PATCH /api/drives/{id}/members/{subject_id}` (role change).
    ///
    /// `set_role` is idempotent — `(subject, resource)` is unique — so the
    /// two HTTP shapes share one service method. Returns the resulting grant.
    pub async fn set_member_role(
        &self,
        caller_id: Uuid,
        drive_id: Uuid,
        subject: Subject,
        role: Role,
        expires_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<Grant, DomainError> {
        let resource = Resource::Drive(drive_id);
        self.authz
            .require(Subject::User(caller_id), Permission::Manage, resource)
            .await?;

        self.refuse_if_personal(drive_id, "set_member_role").await?;

        // Demotion of the last owner = last-owner protection trips. A fresh
        // owner-role write or any non-owner subject is fine; only the case
        // "this subject is currently the only owner AND the new role is not
        // owner" is refused.
        if !matches!(role, Role::Owner) {
            self.refuse_if_last_owner_change(drive_id, subject, caller_id)
                .await?;
        }

        self.authz
            .set_role(caller_id, subject, role, resource, expires_at)
            .await
    }

    /// `DELETE /api/drives/{id}/members/{subject_id}`. Idempotent — removing
    /// a subject with no current grant succeeds (matches `clear_role`).
    pub async fn remove_member(
        &self,
        caller_id: Uuid,
        drive_id: Uuid,
        subject: Subject,
    ) -> Result<(), DomainError> {
        let resource = Resource::Drive(drive_id);
        self.authz
            .require(Subject::User(caller_id), Permission::Manage, resource)
            .await?;

        self.refuse_if_personal(drive_id, "remove_member").await?;

        self.refuse_if_last_owner_change(drive_id, subject, caller_id)
            .await?;

        self.authz.clear_role(subject, resource).await
    }

    // ── Business rules ──────────────────────────────────────────────────────

    /// Personal drives are single-user single-owner; any member mutation is
    /// a category error (§2). Returns `Forbidden` with an audit line.
    async fn refuse_if_personal(&self, drive_id: Uuid, op: &str) -> Result<(), DomainError> {
        let drive = self.drive_repo.get_by_id(drive_id).await.map_err(|e| {
            DomainError::internal_error("Drive", format!("Failed to fetch drive: {e:?}"))
        })?;
        if drive.drive.is_personal() {
            tracing::info!(
                target: "audit",
                event = "drive_membership.rejected",
                reason = "personal_drive_immutable",
                operation = %op,
                drive_id = %drive_id,
                "👮🏻‍♂️ refused {op} on personal drive {drive_id}",
            );
            return Err(DomainError::operation_not_supported(
                "Drive",
                "Personal drives have a fixed single-owner membership and cannot be modified.",
            ));
        }
        Ok(())
    }

    /// Refuse the change if `subject` is currently the sole `Owner` on the
    /// drive and the operation would remove or demote them. A shared drive
    /// must always have at least one Owner — otherwise it becomes orphaned
    /// (no one can ever grant permissions again).
    async fn refuse_if_last_owner_change(
        &self,
        drive_id: Uuid,
        subject: Subject,
        caller_id: Uuid,
    ) -> Result<(), DomainError> {
        let resource = Resource::Drive(drive_id);
        let grants = self.authz.list_grants_on_resource(resource).await?;

        // `subject` must currently BE an owner — otherwise no demotion risk.
        let subject_is_owner = grants
            .iter()
            .any(|g| g.subject == subject && matches!(g.role, Role::Owner));
        if !subject_is_owner {
            return Ok(());
        }

        let owner_count = grants
            .iter()
            .filter(|g| matches!(g.role, Role::Owner))
            .count();
        if owner_count <= 1 {
            tracing::info!(
                target: "audit",
                event = "drive_membership.rejected",
                reason = "last_owner",
                drive_id = %drive_id,
                caller_id = %caller_id,
                subject_type = subject.type_str(),
                subject_id = %subject.id(),
                "👮🏻‍♂️ refused last-owner removal on drive {drive_id}",
            );
            return Err(DomainError::validation_error(
                "A shared drive must keep at least one Owner — promote another \
                 member to Owner first, or delete the drive.",
            ));
        }
        Ok(())
    }
}
