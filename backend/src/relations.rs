//! Directed, typed links between two items of the same collection space.
//!
//! A relation is declared by its source item and points at a target in the same space, so a
//! client can model variants (`variant_of`), components (`part_of`) or a plain association
//! (`related`). The composite foreign keys on the space make a cross-space link impossible, and
//! a transfer removes every link that mentions the moved item.

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

use crate::database::{Database, DatabaseError};

/// Upper bound on the relations one item may declare in a single replacement.
pub const MAX_ITEM_RELATIONS: usize = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RelationKind {
    Related,
    VariantOf,
    PartOf,
}

impl RelationKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Related => "related",
            Self::VariantOf => "variant_of",
            Self::PartOf => "part_of",
        }
    }

    #[must_use]
    pub fn from_code(code: &str) -> Option<Self> {
        match code {
            "related" => Some(Self::Related),
            "variant_of" => Some(Self::VariantOf),
            "part_of" => Some(Self::PartOf),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemRelation {
    pub id: Uuid,
    pub space_id: Uuid,
    pub source_item_id: Uuid,
    pub target_item_id: Uuid,
    pub kind: RelationKind,
    pub source_name: String,
    pub target_name: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug)]
pub enum RelationError {
    Query(sqlx::Error),
    InvalidKind,
    ItemNotFound,
    InvalidTarget,
    SelfRelation,
    DuplicateRelation,
    TooManyRelations,
    RevisionConflict,
    ItemInTrash,
    MalformedData,
}

impl From<sqlx::Error> for RelationError {
    fn from(error: sqlx::Error) -> Self {
        Self::Query(error)
    }
}

impl Database {
    /// Replaces every relation an item declares (its outgoing links) in one transaction.
    ///
    /// The current revision is the concurrency guard, the item must not be trashed, and every
    /// target must belong to the same space. An identical selection is a no-op: it keeps the
    /// revision and does not rewrite the rows.
    ///
    /// # Errors
    ///
    /// Returns a validation, missing/foreign item, duplicate, stale revision, trash, or
    /// storage error.
    #[allow(clippy::too_many_lines)]
    pub async fn replace_item_relations(
        &self,
        item_id: Uuid,
        expected_revision: u64,
        entries: &[(Uuid, RelationKind)],
        actor_account_id: Uuid,
    ) -> Result<u64, RelationError> {
        if entries.len() > MAX_ITEM_RELATIONS {
            return Err(RelationError::TooManyRelations);
        }
        let mut desired: BTreeSet<(Uuid, &'static str)> = BTreeSet::new();
        for (target, kind) in entries {
            if *target == item_id {
                return Err(RelationError::SelfRelation);
            }
            if !desired.insert((*target, kind.as_str())) {
                return Err(RelationError::DuplicateRelation);
            }
        }
        let mut tx = self.pool().begin().await?;
        let row = sqlx::query(
            "SELECT space_id, state, revision FROM inventory_items WHERE id = ? FOR UPDATE",
        )
        .bind(item_id.to_string())
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(RelationError::ItemNotFound)?;
        let space_id = parse_uuid(&row, "space_id")?;
        let revision: u64 = row.try_get("revision")?;
        if revision != expected_revision {
            return Err(RelationError::RevisionConflict);
        }
        let state: Vec<u8> = row.try_get("state")?;
        if state == b"trashed" {
            return Err(RelationError::ItemInTrash);
        }
        for (target, _) in entries {
            let exists: Option<Vec<u8>> =
                sqlx::query_scalar("SELECT id FROM inventory_items WHERE id = ? AND space_id = ?")
                    .bind(target.to_string())
                    .bind(space_id.to_string())
                    .fetch_optional(&mut *tx)
                    .await?;
            if exists.is_none() {
                return Err(RelationError::InvalidTarget);
            }
        }
        let current_rows =
            sqlx::query("SELECT target_item_id, kind FROM item_relations WHERE source_item_id = ?")
                .bind(item_id.to_string())
                .fetch_all(&mut *tx)
                .await?;
        let mut current: BTreeSet<(Uuid, String)> = BTreeSet::new();
        for row in &current_rows {
            let target = parse_uuid(row, "target_item_id")?;
            let kind = String::from_utf8(row.try_get("kind")?)
                .map_err(|_| RelationError::MalformedData)?;
            current.insert((target, kind));
        }
        let desired: BTreeSet<(Uuid, String)> = desired
            .into_iter()
            .map(|(target, kind)| (target, kind.to_owned()))
            .collect();
        if current == desired {
            tx.commit().await?;
            return Ok(revision);
        }
        sqlx::query("DELETE FROM item_relations WHERE source_item_id = ?")
            .bind(item_id.to_string())
            .execute(&mut *tx)
            .await?;
        for (target, kind) in entries {
            sqlx::query(
                "INSERT INTO item_relations \
                    (id, space_id, source_item_id, target_item_id, kind, created_by_account_id) \
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(Uuid::now_v7().to_string())
            .bind(space_id.to_string())
            .bind(item_id.to_string())
            .bind(target.to_string())
            .bind(kind.as_str())
            .bind(actor_account_id.to_string())
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query("UPDATE inventory_items SET revision = revision + 1 WHERE id = ?")
            .bind(item_id.to_string())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(revision + 1)
    }

    /// Lists every relation that mentions an item, outgoing or incoming, in its current space.
    ///
    /// # Errors
    ///
    /// Returns a storage or malformed-data error.
    pub async fn item_relations(&self, item_id: Uuid) -> Result<Vec<ItemRelation>, RelationError> {
        let rows = sqlx::query(
            "SELECT r.id, r.space_id, r.source_item_id, r.target_item_id, r.kind, r.created_at, \
                    s.name AS source_name, t.name AS target_name \
             FROM item_relations r \
             JOIN inventory_items s ON s.id = r.source_item_id AND s.space_id = r.space_id \
             JOIN inventory_items t ON t.id = r.target_item_id AND t.space_id = r.space_id \
             WHERE r.source_item_id = ? OR r.target_item_id = ? \
             ORDER BY r.kind, r.created_at, r.id",
        )
        .bind(item_id.to_string())
        .bind(item_id.to_string())
        .fetch_all(self.pool())
        .await?;
        rows.iter().map(decode_relation).collect()
    }
}

/// Removes every link that mentions an item before its space changes in the same transaction.
pub(crate) async fn clear_item_relations_on_transfer(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    item_id: Uuid,
) -> Result<(), DatabaseError> {
    sqlx::query("DELETE FROM item_relations WHERE source_item_id = ? OR target_item_id = ?")
        .bind(item_id.to_string())
        .bind(item_id.to_string())
        .execute(&mut **tx)
        .await
        .map_err(DatabaseError::Query)?;
    Ok(())
}

fn decode_relation(row: &sqlx::mysql::MySqlRow) -> Result<ItemRelation, RelationError> {
    let raw: Vec<u8> = row.try_get("kind")?;
    let kind = std::str::from_utf8(&raw).map_err(|_| RelationError::MalformedData)?;
    Ok(ItemRelation {
        id: parse_uuid(row, "id")?,
        space_id: parse_uuid(row, "space_id")?,
        source_item_id: parse_uuid(row, "source_item_id")?,
        target_item_id: parse_uuid(row, "target_item_id")?,
        kind: RelationKind::from_code(kind).ok_or(RelationError::MalformedData)?,
        source_name: row.try_get("source_name")?,
        target_name: row.try_get("target_name")?,
        created_at: row.try_get("created_at")?,
    })
}

fn parse_uuid(row: &sqlx::mysql::MySqlRow, column: &str) -> Result<Uuid, RelationError> {
    let bytes: Vec<u8> = row.try_get(column)?;
    let value = std::str::from_utf8(&bytes).map_err(|_| RelationError::MalformedData)?;
    Uuid::parse_str(value).map_err(|_| RelationError::MalformedData)
}
