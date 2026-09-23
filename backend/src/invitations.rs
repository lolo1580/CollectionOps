use chrono::{DateTime, Duration, Utc};
use sqlx::Row;
use subtle::ConstantTimeEq;
use uuid::Uuid;

use crate::{
    credentials::{EmailAddress, InvitationToken, PasswordService},
    database::Database,
};

const INVITATION_LIFETIME_DAYS: i64 = 7;

#[derive(Debug, Clone)]
pub struct Invitation {
    pub id: Uuid,
    pub space_id: Uuid,
    pub recipient_email: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub accepted_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
}

pub struct IssuedInvitation {
    pub invitation: Invitation,
    token: InvitationToken,
}

impl IssuedInvitation {
    #[must_use]
    pub fn link(&self) -> String {
        format!(
            "collectionops://invite/{}/{}",
            self.invitation.id,
            self.token.expose_secret()
        )
    }
}

#[derive(Debug)]
pub enum InvitationError {
    NotFound,
    NotOwner,
    AlreadyMember,
    ExistingAccountRequiresLogin,
    AccountMismatch,
    InvalidAccount,
    Storage(sqlx::Error),
    TokenGeneration,
}

impl From<sqlx::Error> for InvitationError {
    fn from(value: sqlx::Error) -> Self {
        Self::Storage(value)
    }
}

fn validate_link(
    row: &sqlx::mysql::MySqlRow,
    token: &InvitationToken,
) -> Result<(), InvitationError> {
    let stored: Vec<u8> = row.try_get("token_fingerprint")?;
    let candidate = token.fingerprint();
    let valid = stored.len() == 32 && bool::from(stored.as_slice().ct_eq(candidate.as_bytes()));
    let expires_at: DateTime<Utc> = row.try_get("expires_at")?;
    let accepted_at: Option<DateTime<Utc>> = row.try_get("accepted_at")?;
    let revoked_at: Option<DateTime<Utc>> = row.try_get("revoked_at")?;
    if !valid || expires_at <= Utc::now() || accepted_at.is_some() || revoked_at.is_some() {
        return Err(InvitationError::NotFound);
    }
    Ok(())
}

async fn audit<'e, E>(
    executor: E,
    space_id: Uuid,
    actor_id: Uuid,
    invitation_id: Uuid,
    event: &str,
) -> Result<(), sqlx::Error>
where
    E: sqlx::Executor<'e, Database = sqlx::MySql>,
{
    sqlx::query("INSERT INTO space_audit_events (id, space_id, actor_account_id, invitation_id, event_code, created_at) VALUES (?, ?, ?, ?, ?, ?)")
        .bind(Uuid::now_v7().to_string())
        .bind(space_id.to_string())
        .bind(actor_id.to_string())
        .bind(invitation_id.to_string())
        .bind(event)
        .bind(Utc::now())
        .execute(executor)
        .await?;
    Ok(())
}

impl Database {
    /// The owner is checked under the space row lock, so ownership cannot change mid-issue.
    ///
    /// # Errors
    ///
    /// Returns a refusal for a non-owner or existing member, or a storage/randomness failure.
    pub async fn issue_invitation(
        &self,
        space_id: Uuid,
        owner_id: Uuid,
        email: &EmailAddress,
    ) -> Result<IssuedInvitation, InvitationError> {
        let token = InvitationToken::generate().map_err(|_| InvitationError::TokenGeneration)?;
        let now = Utc::now();
        let expires_at = now + Duration::days(INVITATION_LIFETIME_DAYS);
        let id = Uuid::now_v7();
        let mut tx = self.pool().begin().await?;

        let owner: Option<Vec<u8>> =
            sqlx::query_scalar("SELECT owner_account_id FROM spaces WHERE id = ? FOR UPDATE")
                .bind(space_id.to_string())
                .fetch_optional(&mut *tx)
                .await?;
        if owner.as_deref() != Some(owner_id.to_string().as_bytes()) {
            return Err(InvitationError::NotOwner);
        }

        let already_member: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM space_memberships m JOIN accounts a ON a.id = m.account_id WHERE m.space_id = ? AND a.email = ?",
        )
        .bind(space_id.to_string())
        .bind(email.as_str())
        .fetch_one(&mut *tx)
        .await?;
        if already_member != 0 {
            return Err(InvitationError::AlreadyMember);
        }

        // Reissuing invalidates and audits all still-open links for this recipient atomically.
        let previous = sqlx::query("SELECT id FROM space_invitations WHERE space_id = ? AND recipient_email = ? AND accepted_at IS NULL AND revoked_at IS NULL FOR UPDATE")
            .bind(space_id.to_string())
            .bind(email.as_str())
            .fetch_all(&mut *tx)
            .await?;
        sqlx::query("UPDATE space_invitations SET revoked_at = ? WHERE space_id = ? AND recipient_email = ? AND accepted_at IS NULL AND revoked_at IS NULL")
            .bind(now)
            .bind(space_id.to_string())
            .bind(email.as_str())
            .execute(&mut *tx)
            .await?;
        for row in previous {
            let raw: Vec<u8> = row.try_get("id")?;
            let previous_id = Uuid::parse_str(
                std::str::from_utf8(&raw).map_err(|_| InvitationError::InvalidAccount)?,
            )
            .map_err(|_| InvitationError::InvalidAccount)?;
            audit(
                &mut *tx,
                space_id,
                owner_id,
                previous_id,
                "invitation_revoked",
            )
            .await?;
        }

        sqlx::query("INSERT INTO space_invitations (id, space_id, recipient_email, token_fingerprint, inviter_account_id, created_at, expires_at) VALUES (?, ?, ?, ?, ?, ?, ?)")
            .bind(id.to_string())
            .bind(space_id.to_string())
            .bind(email.as_str())
            .bind(token.fingerprint().as_bytes().as_slice())
            .bind(owner_id.to_string())
            .bind(now)
            .bind(expires_at)
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO space_invitation_grants (invitation_id, permission_code) VALUES (?, 'collections_read')")
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
        audit(&mut *tx, space_id, owner_id, id, "invitation_created").await?;
        tx.commit().await?;

        Ok(IssuedInvitation {
            invitation: Invitation {
                id,
                space_id,
                recipient_email: email.as_str().to_owned(),
                created_at: now,
                expires_at,
                accepted_at: None,
                revoked_at: None,
            },
            token,
        })
    }

    /// Lists metadata without exposing invitation tokens.
    ///
    /// # Errors
    ///
    /// Returns a refusal for a non-owner or a storage failure.
    pub async fn list_invitations(
        &self,
        space_id: Uuid,
        owner_id: Uuid,
    ) -> Result<Vec<Invitation>, InvitationError> {
        let owner: Option<Vec<u8>> =
            sqlx::query_scalar("SELECT owner_account_id FROM spaces WHERE id = ?")
                .bind(space_id.to_string())
                .fetch_optional(self.pool())
                .await?;
        if owner.as_deref() != Some(owner_id.to_string().as_bytes()) {
            return Err(InvitationError::NotOwner);
        }
        let rows = sqlx::query("SELECT id, recipient_email, created_at, expires_at, accepted_at, revoked_at FROM space_invitations WHERE space_id = ? ORDER BY created_at DESC")
            .bind(space_id.to_string())
            .fetch_all(self.pool())
            .await?;
        rows.iter()
            .map(|row| {
                let id: Vec<u8> = row.try_get("id")?;
                Ok(Invitation {
                    id: Uuid::parse_str(
                        std::str::from_utf8(&id).map_err(|_| InvitationError::InvalidAccount)?,
                    )
                    .map_err(|_| InvitationError::InvalidAccount)?,
                    space_id,
                    recipient_email: row.try_get("recipient_email")?,
                    created_at: row.try_get("created_at")?,
                    expires_at: row.try_get("expires_at")?,
                    accepted_at: row.try_get("accepted_at")?,
                    revoked_at: row.try_get("revoked_at")?,
                })
            })
            .collect()
    }

    /// Revokes one pending invitation and audits the change.
    ///
    /// # Errors
    ///
    /// Returns a refusal for a non-owner, missing/used invitation, or storage failure.
    pub async fn revoke_invitation(
        &self,
        space_id: Uuid,
        invitation_id: Uuid,
        owner_id: Uuid,
    ) -> Result<(), InvitationError> {
        let mut tx = self.pool().begin().await?;
        let owner: Option<Vec<u8>> =
            sqlx::query_scalar("SELECT owner_account_id FROM spaces WHERE id = ? FOR UPDATE")
                .bind(space_id.to_string())
                .fetch_optional(&mut *tx)
                .await?;
        if owner.as_deref() != Some(owner_id.to_string().as_bytes()) {
            return Err(InvitationError::NotOwner);
        }
        let affected = sqlx::query("UPDATE space_invitations SET revoked_at = ? WHERE id = ? AND space_id = ? AND accepted_at IS NULL AND revoked_at IS NULL")
            .bind(Utc::now()).bind(invitation_id.to_string()).bind(space_id.to_string())
            .execute(&mut *tx).await?.rows_affected();
        if affected != 1 {
            return Err(InvitationError::NotFound);
        }
        audit(
            &mut *tx,
            space_id,
            owner_id,
            invitation_id,
            "invitation_revoked",
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Accepts once, in one transaction. New accounts are created only from a valid emailed link.
    ///
    /// # Errors
    ///
    /// Returns a refusal for an invalid link, mismatched account, existing membership, invalid
    /// new-account credentials, or a storage failure.
    pub async fn accept_invitation(
        &self,
        invitation_id: Uuid,
        token: &InvitationToken,
        signed_in_account: Option<Uuid>,
        new_account: Option<(&str, &str)>,
        passwords: &PasswordService,
    ) -> Result<Uuid, InvitationError> {
        // A cheap read rejects invalid links before Argon2id, preventing unauthenticated callers
        // from spending memory and CPU on arbitrary invitation identifiers. The check is repeated
        // under the row lock after hashing so revocation/expiry cannot race acceptance.
        let preflight = sqlx::query("SELECT token_fingerprint, expires_at, accepted_at, revoked_at FROM space_invitations WHERE id = ?")
            .bind(invitation_id.to_string()).fetch_optional(self.pool()).await?
            .ok_or(InvitationError::NotFound)?;
        validate_link(&preflight, token)?;

        // Hash before taking the row lock; this is intentionally expensive.
        let new_hash = new_account
            .map(|(_, password)| {
                passwords
                    .hash_password(password)
                    .map_err(|_| InvitationError::InvalidAccount)
            })
            .transpose()?;
        let mut tx = self.pool().begin().await?;
        let row = sqlx::query("SELECT space_id, recipient_email, token_fingerprint, expires_at, accepted_at, revoked_at FROM space_invitations WHERE id = ? FOR UPDATE")
            .bind(invitation_id.to_string()).fetch_optional(&mut *tx).await?
            .ok_or(InvitationError::NotFound)?;
        validate_link(&row, token)?;
        let space_bytes: Vec<u8> = row.try_get("space_id")?;
        let space_id = Uuid::parse_str(
            std::str::from_utf8(&space_bytes).map_err(|_| InvitationError::InvalidAccount)?,
        )
        .map_err(|_| InvitationError::InvalidAccount)?;
        let recipient_email: String = row.try_get("recipient_email")?;
        let existing =
            sqlx::query("SELECT id, email_verified_at FROM accounts WHERE email = ? FOR UPDATE")
                .bind(&recipient_email)
                .fetch_optional(&mut *tx)
                .await?;
        let account_id = if let Some(existing) = existing {
            let id: Vec<u8> = existing.try_get("id")?;
            let id = Uuid::parse_str(
                std::str::from_utf8(&id).map_err(|_| InvitationError::InvalidAccount)?,
            )
            .map_err(|_| InvitationError::InvalidAccount)?;
            if signed_in_account != Some(id) {
                return Err(InvitationError::ExistingAccountRequiresLogin);
            }
            let verified: Option<DateTime<Utc>> = existing.try_get("email_verified_at")?;
            if verified.is_none() {
                return Err(InvitationError::AccountMismatch);
            }
            id
        } else {
            if signed_in_account.is_some() {
                return Err(InvitationError::AccountMismatch);
            }
            let (name, _) = new_account.ok_or(InvitationError::InvalidAccount)?;
            let name = name.trim();
            if name.is_empty() || name.chars().count() > 255 {
                return Err(InvitationError::InvalidAccount);
            }
            let id = Uuid::now_v7();
            sqlx::query("INSERT INTO accounts (id, display_name, email, password_hash, email_verified_at) VALUES (?, ?, ?, ?, ?)")
                .bind(id.to_string()).bind(name).bind(&recipient_email)
                .bind(new_hash.as_ref().ok_or(InvitationError::InvalidAccount)?.as_str())
                .bind(Utc::now()).execute(&mut *tx).await?;
            id
        };
        let member: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM space_memberships WHERE space_id = ? AND account_id = ?",
        )
        .bind(space_id.to_string())
        .bind(account_id.to_string())
        .fetch_one(&mut *tx)
        .await?;
        if member != 0 {
            return Err(InvitationError::AlreadyMember);
        }
        sqlx::query("INSERT INTO space_memberships (space_id, account_id) VALUES (?, ?)")
            .bind(space_id.to_string())
            .bind(account_id.to_string())
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO space_permission_grants (space_id, account_id, permission_code) SELECT ?, ?, permission_code FROM space_invitation_grants WHERE invitation_id = ?")
            .bind(space_id.to_string()).bind(account_id.to_string()).bind(invitation_id.to_string())
            .execute(&mut *tx).await?;
        sqlx::query(
            "UPDATE space_invitations SET accepted_account_id = ?, accepted_at = ? WHERE id = ?",
        )
        .bind(account_id.to_string())
        .bind(Utc::now())
        .bind(invitation_id.to_string())
        .execute(&mut *tx)
        .await?;
        audit(
            &mut *tx,
            space_id,
            account_id,
            invitation_id,
            "invitation_accepted",
        )
        .await?;
        tx.commit().await?;
        Ok(account_id)
    }
}
