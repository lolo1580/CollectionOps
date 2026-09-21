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
}

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

    use super::{Permission, Principal};

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
}
