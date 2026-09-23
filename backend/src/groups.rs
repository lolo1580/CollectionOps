//! Named series and groupings scoped to one collection space.

use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

use crate::database::{Database, DatabaseError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventoryGroup {
    pub id: Uuid,
    pub space_id: Uuid,
    pub kind: String,
    pub name: String,
    pub revision: u64,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug)]
pub enum GroupError {
    Query(sqlx::Error),
    InvalidName,
    InvalidKind,
    DuplicateName,
    GroupNotEmpty,
    SpaceNotFound,
    ItemNotFound,
    GroupNotFound,
    InvalidGroup,
    RevisionConflict,
    ItemInTrash,
    MalformedData,
}

impl From<sqlx::Error> for GroupError {
    fn from(error: sqlx::Error) -> Self {
        Self::Query(error)
    }
}

impl Database {
    /// Creates a series or grouping in a space.
    ///
    /// # Errors
    ///
    /// Returns validation, duplicate, missing-space, or storage errors.
    pub async fn create_group(
        &self,
        space_id: Uuid,
        kind: &str,
        name: &str,
    ) -> Result<InventoryGroup, GroupError> {
        let name = name.trim();
        if !matches!(kind, "series" | "group") {
            return Err(GroupError::InvalidKind);
        }
        if name.is_empty() || name.chars().count() > 255 {
            return Err(GroupError::InvalidName);
        }
        let space: Option<Vec<u8>> = sqlx::query_scalar("SELECT id FROM spaces WHERE id = ?")
            .bind(space_id.to_string())
            .fetch_optional(self.pool())
            .await?;
        if space.is_none() {
            return Err(GroupError::SpaceNotFound);
        }
        let id = Uuid::now_v7();
        sqlx::query("INSERT INTO inventory_groups (id, space_id, kind, name) VALUES (?, ?, ?, ?)")
            .bind(id.to_string())
            .bind(space_id.to_string())
            .bind(kind)
            .bind(name)
            .execute(self.pool())
            .await
            .map_err(|error| {
                if matches!(&error, sqlx::Error::Database(db) if db.is_unique_violation()) {
                    GroupError::DuplicateName
                } else {
                    GroupError::Query(error)
                }
            })?;
        self.group(space_id, id).await
    }

    /// Loads a group only when it belongs to the requested space.
    ///
    /// # Errors
    ///
    /// Returns not-found or storage errors.
    pub async fn group(
        &self,
        space_id: Uuid,
        group_id: Uuid,
    ) -> Result<InventoryGroup, GroupError> {
        let row = sqlx::query("SELECT id, space_id, kind, name, revision, created_at FROM inventory_groups WHERE space_id = ? AND id = ?")
            .bind(space_id.to_string()).bind(group_id.to_string())
            .fetch_optional(self.pool()).await?.ok_or(GroupError::GroupNotFound)?;
        decode_group(&row)
    }

    /// Lists all series and groupings in one space.
    ///
    /// # Errors
    ///
    /// Returns a storage or malformed-data error.
    pub async fn groups_in_space(&self, space_id: Uuid) -> Result<Vec<InventoryGroup>, GroupError> {
        let rows = sqlx::query("SELECT id, space_id, kind, name, revision, created_at FROM inventory_groups WHERE space_id = ? ORDER BY kind, name, id")
            .bind(space_id.to_string()).fetch_all(self.pool()).await?;
        rows.iter().map(decode_group).collect()
    }

    /// Lists groups still belonging to the item's current space.
    ///
    /// # Errors
    ///
    /// Returns a storage or malformed-data error.
    pub async fn item_groups(&self, item_id: Uuid) -> Result<Vec<InventoryGroup>, GroupError> {
        let rows = sqlx::query(
            "SELECT g.id, g.space_id, g.kind, g.name, g.revision, g.created_at FROM inventory_groups g \
             JOIN inventory_group_members m ON m.group_id = g.id AND m.space_id = g.space_id \
             JOIN inventory_items i ON i.id = m.item_id AND i.space_id = m.space_id \
             WHERE i.id = ? ORDER BY g.kind, g.name, g.id",
        )
        .bind(item_id.to_string())
        .fetch_all(self.pool())
        .await?;
        rows.iter().map(decode_group).collect()
    }

    /// Replaces all memberships atomically, using the item's revision as a concurrency guard.
    ///
    /// # Errors
    ///
    /// Returns validation, missing item/group, stale revision, forbidden state, or storage errors.
    pub async fn replace_item_groups(
        &self,
        item_id: Uuid,
        group_ids: &[Uuid],
        expected_revision: u64,
    ) -> Result<u64, GroupError> {
        let mut tx = self.pool().begin().await?;
        let row = sqlx::query(
            "SELECT space_id, state, revision FROM inventory_items WHERE id = ? FOR UPDATE",
        )
        .bind(item_id.to_string())
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(GroupError::ItemNotFound)?;
        let space_id = parse_uuid(&row, "space_id")?;
        let revision: u64 = row.try_get("revision")?;
        if revision != expected_revision {
            return Err(GroupError::RevisionConflict);
        }
        let state: Vec<u8> = row.try_get("state")?;
        if state == b"trashed" {
            return Err(GroupError::ItemInTrash);
        }
        let mut unique = group_ids.to_vec();
        unique.sort_unstable();
        unique.dedup();
        for group_id in &unique {
            let exists: Option<Vec<u8>> =
                sqlx::query_scalar("SELECT id FROM inventory_groups WHERE id = ? AND space_id = ?")
                    .bind(group_id.to_string())
                    .bind(space_id.to_string())
                    .fetch_optional(&mut *tx)
                    .await?;
            if exists.is_none() {
                return Err(GroupError::InvalidGroup);
            }
        }
        let current_rows = sqlx::query(
            "SELECT group_id FROM inventory_group_members WHERE item_id = ? ORDER BY group_id",
        )
        .bind(item_id.to_string())
        .fetch_all(&mut *tx)
        .await?;
        let mut current = current_rows
            .iter()
            .map(|row| parse_uuid(row, "group_id"))
            .collect::<Result<Vec<_>, _>>()?;
        current.sort_unstable();
        if current == unique {
            return Ok(revision);
        }
        sqlx::query("DELETE FROM inventory_group_members WHERE item_id = ?")
            .bind(item_id.to_string())
            .execute(&mut *tx)
            .await?;
        for group_id in unique {
            sqlx::query("INSERT INTO inventory_group_members (space_id, group_id, item_id) VALUES (?, ?, ?)")
                .bind(space_id.to_string()).bind(group_id.to_string()).bind(item_id.to_string())
                .execute(&mut *tx).await?;
        }
        sqlx::query("UPDATE inventory_items SET revision = revision + 1 WHERE id = ?")
            .bind(item_id.to_string())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(revision + 1)
    }

    /// Renames a series or grouping without changing its identity or assignments.
    ///
    /// # Errors
    ///
    /// Returns a validation, duplicate, not-found, stale revision, or storage error.
    pub async fn rename_group(
        &self,
        space_id: Uuid,
        group_id: Uuid,
        name: &str,
        expected_revision: u64,
    ) -> Result<InventoryGroup, GroupError> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 255 {
            return Err(GroupError::InvalidName);
        }
        let mut tx = self.pool().begin().await?;
        let row = sqlx::query(
            "SELECT id, space_id, kind, name, revision, created_at FROM inventory_groups \
             WHERE space_id = ? AND id = ? FOR UPDATE",
        )
        .bind(space_id.to_string())
        .bind(group_id.to_string())
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(GroupError::GroupNotFound)?;
        let revision: u64 = row.try_get("revision")?;
        if revision != expected_revision {
            return Err(GroupError::RevisionConflict);
        }
        let current_name: String = row.try_get("name")?;
        if current_name == name {
            let group = decode_group(&row)?;
            tx.commit().await?;
            return Ok(group);
        }
        sqlx::query("UPDATE inventory_groups SET name = ?, revision = revision + 1 WHERE space_id = ? AND id = ?")
            .bind(name).bind(space_id.to_string()).bind(group_id.to_string())
            .execute(&mut *tx).await.map_err(map_duplicate)?;
        let updated = sqlx::query(
            "SELECT id, space_id, kind, name, revision, created_at FROM inventory_groups \
             WHERE space_id = ? AND id = ?",
        )
        .bind(space_id.to_string())
        .bind(group_id.to_string())
        .fetch_one(&mut *tx)
        .await?;
        let group = decode_group(&updated)?;
        tx.commit().await?;
        Ok(group)
    }

    /// Deletes an empty group; memberships must be explicitly removed first.
    ///
    /// # Errors
    ///
    /// Returns not-found, non-empty, stale revision, or storage errors.
    pub async fn delete_group(
        &self,
        space_id: Uuid,
        group_id: Uuid,
        expected_revision: u64,
    ) -> Result<(), GroupError> {
        let mut tx = self.pool().begin().await?;
        let row = sqlx::query(
            "SELECT revision FROM inventory_groups WHERE space_id = ? AND id = ? FOR UPDATE",
        )
        .bind(space_id.to_string())
        .bind(group_id.to_string())
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(GroupError::GroupNotFound)?;
        let revision: u64 = row.try_get("revision")?;
        if revision != expected_revision {
            return Err(GroupError::RevisionConflict);
        }
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM inventory_group_members WHERE group_id = ?")
                .bind(group_id.to_string())
                .fetch_one(&mut *tx)
                .await?;
        if count > 0 {
            return Err(GroupError::GroupNotEmpty);
        }
        sqlx::query("DELETE FROM inventory_groups WHERE space_id = ? AND id = ?")
            .bind(space_id.to_string())
            .bind(group_id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(|error| {
                if matches!(&error, sqlx::Error::Database(db) if db.is_foreign_key_violation()) {
                    GroupError::GroupNotEmpty
                } else {
                    GroupError::Query(error)
                }
            })?;
        tx.commit().await?;
        Ok(())
    }
}

/// Removes source-space memberships before an item's space is changed in the same transaction.
pub(crate) async fn clear_item_groups_on_transfer(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    item_id: Uuid,
) -> Result<(), DatabaseError> {
    sqlx::query("DELETE FROM inventory_group_members WHERE item_id = ?")
        .bind(item_id.to_string())
        .execute(&mut **tx)
        .await
        .map_err(DatabaseError::Query)?;
    Ok(())
}

fn decode_group(row: &sqlx::mysql::MySqlRow) -> Result<InventoryGroup, GroupError> {
    Ok(InventoryGroup {
        id: parse_uuid(row, "id")?,
        space_id: parse_uuid(row, "space_id")?,
        kind: String::from_utf8(row.try_get("kind")?).map_err(|_| GroupError::MalformedData)?,
        name: row.try_get("name")?,
        revision: row.try_get("revision")?,
        created_at: row.try_get("created_at")?,
    })
}

fn map_duplicate(error: sqlx::Error) -> GroupError {
    if matches!(&error, sqlx::Error::Database(db) if db.is_unique_violation()) {
        GroupError::DuplicateName
    } else {
        GroupError::Query(error)
    }
}

fn parse_uuid(row: &sqlx::mysql::MySqlRow, column: &str) -> Result<Uuid, GroupError> {
    let bytes: Vec<u8> = row.try_get(column)?;
    let value = std::str::from_utf8(&bytes).map_err(|_| GroupError::MalformedData)?;
    Uuid::parse_str(value).map_err(|_| GroupError::MalformedData)
}
