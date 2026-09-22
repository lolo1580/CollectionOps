use std::{collections::BTreeSet, error::Error, fmt};

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    CollectionsRead,
    CollectionsWrite,
    AcquisitionsRead,
    AcquisitionsWrite,
    FinanceRead,
    FinanceWrite,
    DocumentsRead,
    DocumentsWrite,
    ReferentialRead,
    ReferentialWrite,
    SynchronizationUse,
    UsersManage,
    AuditRead,
    AdministrationManage,
}

impl Permission {
    #[must_use]
    pub const fn is_space_scoped(self) -> bool {
        matches!(
            self,
            Self::CollectionsRead
                | Self::CollectionsWrite
                | Self::AcquisitionsRead
                | Self::AcquisitionsWrite
                | Self::FinanceRead
                | Self::FinanceWrite
                | Self::DocumentsRead
                | Self::DocumentsWrite
        )
    }
}

/// Membership loaded by the backend for one specific space and account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpaceMembership {
    pub space_id: Uuid,
    pub account_id: Uuid,
    pub permissions: BTreeSet<Permission>,
}

/// Current owner loaded by the backend for one specific space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpaceOwnership {
    pub space_id: Uuid,
    pub owner_account_id: Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Principal {
    pub subject: Uuid,
    pub display_name: String,
    pub roles: BTreeSet<String>,
    pub permissions: BTreeSet<Permission>,
}

impl Principal {
    #[must_use]
    pub fn has_permission(&self, permission: Permission) -> bool {
        self.permissions.contains(&permission)
    }

    /// Verifies that the principal owns an effective permission.
    ///
    /// # Errors
    ///
    /// Returns [`AuthorizationError`] when the permission is absent. Role expansion must happen
    /// before constructing the principal, so this check never grants implicit privileges.
    pub fn require_permission(&self, permission: Permission) -> Result<(), AuthorizationError> {
        if self.has_permission(permission) {
            Ok(())
        } else {
            Err(AuthorizationError { permission })
        }
    }

    /// Requires both an application permission and an explicit grant in the requested space.
    /// The membership must be loaded from trusted storage, not supplied by the client.
    ///
    /// # Errors
    ///
    /// Returns [`SpaceAuthorizationError`] for a missing or mismatched membership, a missing
    /// grant, or a permission that cannot be scoped to a space.
    pub fn require_space_permission(
        &self,
        space_id: Uuid,
        membership: Option<&SpaceMembership>,
        permission: Permission,
    ) -> Result<(), SpaceAuthorizationError> {
        if !permission.is_space_scoped() || !self.has_permission(permission) {
            return Err(SpaceAuthorizationError);
        }

        match membership {
            Some(membership)
                if membership.space_id == space_id
                    && membership.account_id == self.subject
                    && membership.permissions.contains(&permission) =>
            {
                Ok(())
            }
            _ => Err(SpaceAuthorizationError),
        }
    }

    /// Requires ownership of the requested space, for invitation management.
    /// The ownership must be loaded from trusted storage for the requested space.
    ///
    /// # Errors
    ///
    /// Returns [`SpaceAuthorizationError`] when this principal is not the space owner.
    pub fn require_space_owner(
        &self,
        space_id: Uuid,
        ownership: &SpaceOwnership,
    ) -> Result<(), SpaceAuthorizationError> {
        if ownership.space_id == space_id && self.subject == ownership.owner_account_id {
            Ok(())
        } else {
            Err(SpaceAuthorizationError)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpaceAuthorizationError;

impl fmt::Display for SpaceAuthorizationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("space access denied")
    }
}

impl Error for SpaceAuthorizationError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthorizationError {
    permission: Permission,
}

impl AuthorizationError {
    #[must_use]
    pub const fn permission(self) -> Permission {
        self.permission
    }
}

impl fmt::Display for AuthorizationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "missing permission: {:?}", self.permission)
    }
}

impl Error for AuthorizationError {}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use uuid::Uuid;

    use super::{Permission, Principal, SpaceMembership, SpaceOwnership};

    fn principal_with(permissions: impl IntoIterator<Item = Permission>) -> Principal {
        Principal {
            subject: Uuid::nil(),
            display_name: "Test".to_owned(),
            roles: BTreeSet::new(),
            permissions: permissions.into_iter().collect(),
        }
    }

    #[test]
    fn explicit_permission_is_granted() {
        let principal = principal_with([Permission::CollectionsRead]);

        assert!(
            principal
                .require_permission(Permission::CollectionsRead)
                .is_ok()
        );
    }

    #[test]
    fn absent_permission_is_denied() {
        let principal = principal_with([Permission::CollectionsRead]);

        let error = principal
            .require_permission(Permission::FinanceRead)
            .expect_err("financial access must not be inferred from collection access");

        assert_eq!(error.permission(), Permission::FinanceRead);
    }

    #[test]
    fn space_grant_requires_matching_account_space_and_application_permission() {
        let account_id = Uuid::now_v7();
        let space_id = Uuid::now_v7();
        let principal = Principal {
            subject: account_id,
            display_name: "Test".to_owned(),
            roles: BTreeSet::new(),
            permissions: [Permission::CollectionsRead].into(),
        };
        let mut membership = SpaceMembership {
            space_id,
            account_id,
            permissions: [Permission::CollectionsRead].into(),
        };

        assert!(
            principal
                .require_space_permission(space_id, Some(&membership), Permission::CollectionsRead)
                .is_ok()
        );
        membership.permissions.insert(Permission::CollectionsWrite);
        assert!(
            principal
                .require_space_permission(space_id, Some(&membership), Permission::CollectionsWrite)
                .is_err()
        );
        assert!(
            principal
                .require_space_permission(
                    Uuid::now_v7(),
                    Some(&membership),
                    Permission::CollectionsRead
                )
                .is_err()
        );
        assert!(
            principal
                .require_space_permission(space_id, None, Permission::CollectionsRead)
                .is_err()
        );

        membership.account_id = Uuid::now_v7();
        assert!(
            principal
                .require_space_permission(space_id, Some(&membership), Permission::CollectionsRead)
                .is_err()
        );
    }

    #[test]
    fn financial_and_global_permissions_are_never_inferred_from_collection_grants() {
        let account_id = Uuid::now_v7();
        let space_id = Uuid::now_v7();
        let principal = Principal {
            subject: account_id,
            display_name: "Test".to_owned(),
            roles: BTreeSet::new(),
            permissions: [Permission::CollectionsRead, Permission::FinanceRead].into(),
        };
        let membership = SpaceMembership {
            space_id,
            account_id,
            permissions: [
                Permission::CollectionsRead,
                Permission::AdministrationManage,
            ]
            .into(),
        };

        assert!(
            principal
                .require_space_permission(space_id, Some(&membership), Permission::FinanceRead)
                .is_err()
        );
        assert!(
            principal
                .require_space_permission(
                    space_id,
                    Some(&membership),
                    Permission::AdministrationManage
                )
                .is_err()
        );
    }

    #[test]
    fn invitation_management_requires_current_owner() {
        let principal = principal_with([]);
        let space_id = Uuid::now_v7();
        let ownership = SpaceOwnership {
            space_id,
            owner_account_id: principal.subject,
        };

        assert!(principal.require_space_owner(space_id, &ownership).is_ok());
        assert!(
            principal
                .require_space_owner(Uuid::now_v7(), &ownership)
                .is_err()
        );
        assert!(
            principal_with([])
                .require_space_owner(
                    space_id,
                    &SpaceOwnership {
                        owner_account_id: Uuid::now_v7(),
                        ..ownership
                    }
                )
                .is_err()
        );
    }
}
