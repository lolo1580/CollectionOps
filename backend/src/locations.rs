//! Hierarchical, space-scoped physical locations and item movement history.

use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

use crate::database::{Database, DatabaseError};

const ROOT_PARENT_KEY: &str = "00000000-0000-0000-0000-000000000000";
const MAX_LOCATION_NAME_CHARS: usize = 255;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    pub id: Uuid,
    pub space_id: Uuid,
    pub parent_id: Option<Uuid>,
    pub name: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemLocationEvent {
    pub id: Uuid,
    pub from_location_id: Option<Uuid>,
    pub from_location_name: Option<String>,
    pub to_location_id: Option<Uuid>,
    pub to_location_name: Option<String>,
    pub actor_display_name: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug)]
pub enum LocationError {
    Query(sqlx::Error),
    InvalidName,
    SpaceNotFound,
    ParentNotFound,
    LocationNotFound,
    DuplicateName,
    ItemNotFound,
    RevisionConflict,
    ItemInTrash,
    MalformedData,
}

impl From<sqlx::Error> for LocationError {
    fn from(error: sqlx::Error) -> Self {
        Self::Query(error)
    }
}

impl Database {
    /// Creates a root location or child location inside the same space.
    ///
    /// # Errors
    ///
    /// Returns a validation, missing space/parent, duplicate, or query error.
    pub async fn create_location(
        &self,
        space_id: Uuid,
        parent_id: Option<Uuid>,
        name: &str,
    ) -> Result<Location, LocationError> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > MAX_LOCATION_NAME_CHARS {
            return Err(LocationError::InvalidName);
        }
        let mut tx = self.pool().begin().await?;
        let space: Option<Vec<u8>> =
            sqlx::query_scalar("SELECT id FROM spaces WHERE id = ? FOR UPDATE")
                .bind(space_id.to_string())
                .fetch_optional(&mut *tx)
                .await?;
        if space.is_none() {
            return Err(LocationError::SpaceNotFound);
        }
        if let Some(parent_id) = parent_id {
            let parent: Option<Vec<u8>> = sqlx::query_scalar(
                "SELECT id FROM inventory_locations WHERE id = ? AND space_id = ?",
            )
            .bind(parent_id.to_string())
            .bind(space_id.to_string())
            .fetch_optional(&mut *tx)
            .await?;
            if parent.is_none() {
                return Err(LocationError::ParentNotFound);
            }
        }
        let parent_key = parent_id.map_or_else(|| ROOT_PARENT_KEY.to_owned(), |id| id.to_string());
        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO inventory_locations (id, space_id, parent_id, parent_key, name) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(id.to_string())
        .bind(space_id.to_string())
        .bind(parent_id.map(|id| id.to_string()))
        .bind(parent_key)
        .bind(name)
        .execute(&mut *tx)
        .await
        .map_err(map_duplicate)?;
        tx.commit().await?;
        self.location(space_id, id).await
    }

    /// Lists all locations in an authorized space.
    ///
    /// # Errors
    ///
    /// Returns a query or malformed-data error.
    pub async fn locations_in_space(&self, space_id: Uuid) -> Result<Vec<Location>, LocationError> {
        let rows = sqlx::query(
            "SELECT id, space_id, parent_id, name, created_at \
             FROM inventory_locations WHERE space_id = ? ORDER BY name, id",
        )
        .bind(space_id.to_string())
        .fetch_all(self.pool())
        .await?;
        rows.iter().map(decode_location).collect()
    }

    /// Loads a location only within its own space.
    ///
    /// # Errors
    ///
    /// Returns not-found, query, or malformed-data errors.
    pub async fn location(&self, space_id: Uuid, id: Uuid) -> Result<Location, LocationError> {
        let row = sqlx::query(
            "SELECT id, space_id, parent_id, name, created_at \
             FROM inventory_locations WHERE id = ? AND space_id = ?",
        )
        .bind(id.to_string())
        .bind(space_id.to_string())
        .fetch_optional(self.pool())
        .await?
        .ok_or(LocationError::LocationNotFound)?;
        decode_location(&row)
    }

    /// Reads the item's sole current location, if any.
    ///
    /// # Errors
    ///
    /// Returns not-found, query, or malformed-data errors.
    pub async fn current_item_location(
        &self,
        item_id: Uuid,
    ) -> Result<Option<Location>, LocationError> {
        let row =
            sqlx::query("SELECT space_id, current_location_id FROM inventory_items WHERE id = ?")
                .bind(item_id.to_string())
                .fetch_optional(self.pool())
                .await?
                .ok_or(LocationError::ItemNotFound)?;
        let space_id = decode_uuid(&row, "space_id")?;
        let location_id = decode_optional_uuid(&row, "current_location_id")?;
        match location_id {
            Some(id) => self.location(space_id, id).await.map(Some),
            None => Ok(None),
        }
    }

    /// Moves an item inside its current space or removes its location. A no-op does not
    /// change the revision or append a history event.
    ///
    /// # Errors
    ///
    /// Returns a missing item/location, stale revision, trash, or query error.
    pub async fn move_item_to_location(
        &self,
        item_id: Uuid,
        to_location_id: Option<Uuid>,
        expected_revision: u64,
        actor_account_id: Uuid,
    ) -> Result<u64, LocationError> {
        let mut tx = self.pool().begin().await?;
        let row = sqlx::query(
            "SELECT space_id, current_location_id, state, revision \
             FROM inventory_items WHERE id = ? FOR UPDATE",
        )
        .bind(item_id.to_string())
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(LocationError::ItemNotFound)?;
        let space_id = decode_uuid(&row, "space_id")?;
        let from_location_id = decode_optional_uuid(&row, "current_location_id")?;
        let state: Vec<u8> = row.try_get("state")?;
        if state == b"trashed" {
            return Err(LocationError::ItemInTrash);
        }
        let revision: u64 = row.try_get("revision")?;
        if revision != expected_revision {
            return Err(LocationError::RevisionConflict);
        }
        if let Some(id) = to_location_id {
            let belongs: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM inventory_locations WHERE id = ? AND space_id = ?",
            )
            .bind(id.to_string())
            .bind(space_id.to_string())
            .fetch_one(&mut *tx)
            .await?;
            if belongs == 0 {
                return Err(LocationError::LocationNotFound);
            }
        }
        if from_location_id == to_location_id {
            tx.commit().await?;
            return Ok(revision);
        }
        insert_location_event(
            &mut tx,
            item_id,
            space_id,
            from_location_id,
            to_location_id,
            actor_account_id,
        )
        .await?;
        sqlx::query(
            "UPDATE inventory_items SET current_location_id = ?, revision = revision + 1 \
             WHERE id = ?",
        )
        .bind(to_location_id.map(|id| id.to_string()))
        .bind(item_id.to_string())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(revision + 1)
    }

    /// Lists movement events only from the item's current space.
    ///
    /// # Errors
    ///
    /// Returns a query or malformed-data error.
    pub async fn item_location_events(
        &self,
        item_id: Uuid,
        space_id: Uuid,
    ) -> Result<Vec<ItemLocationEvent>, LocationError> {
        let rows = sqlx::query(
            "SELECT e.id, e.from_location_id, origin.name AS from_location_name, \
                    e.to_location_id, destination.name AS to_location_name, \
                    actor.display_name AS actor_display_name, e.created_at \
             FROM item_location_events e \
             LEFT JOIN inventory_locations origin ON origin.id = e.from_location_id \
             LEFT JOIN inventory_locations destination ON destination.id = e.to_location_id \
             JOIN accounts actor ON actor.id = e.actor_account_id \
             WHERE e.item_id = ? AND e.space_id = ? \
             ORDER BY e.created_at DESC, e.id DESC",
        )
        .bind(item_id.to_string())
        .bind(space_id.to_string())
        .fetch_all(self.pool())
        .await?;
        rows.iter()
            .map(|row| {
                Ok(ItemLocationEvent {
                    id: decode_uuid(row, "id")?,
                    from_location_id: decode_optional_uuid(row, "from_location_id")?,
                    from_location_name: row.try_get("from_location_name")?,
                    to_location_id: decode_optional_uuid(row, "to_location_id")?,
                    to_location_name: row.try_get("to_location_name")?,
                    actor_display_name: row.try_get("actor_display_name")?,
                    created_at: row.try_get("created_at")?,
                })
            })
            .collect()
    }
}

/// Records the departure from a source-space location before a cross-space transfer.
/// The caller clears `current_location_id` in the same transaction.
pub(crate) async fn record_transfer_location_exit(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    item_id: Uuid,
    source_space_id: Uuid,
    from_location_raw: Option<Vec<u8>>,
    actor_account_id: Uuid,
) -> Result<(), DatabaseError> {
    if let Some(raw) = from_location_raw {
        let from_location_id = std::str::from_utf8(&raw)
            .map_err(|error| DatabaseError::Query(sqlx::Error::Decode(error.into())))
            .and_then(|value| {
                Uuid::parse_str(value)
                    .map_err(|error| DatabaseError::Query(sqlx::Error::Decode(error.into())))
            })?;
        insert_location_event(
            tx,
            item_id,
            source_space_id,
            Some(from_location_id),
            None,
            actor_account_id,
        )
        .await
        .map_err(|error| match error {
            LocationError::Query(query) => DatabaseError::Query(query),
            _ => unreachable!("event insert only returns storage errors"),
        })?;
    }
    Ok(())
}

async fn insert_location_event(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    item_id: Uuid,
    space_id: Uuid,
    from_location_id: Option<Uuid>,
    to_location_id: Option<Uuid>,
    actor_account_id: Uuid,
) -> Result<(), LocationError> {
    sqlx::query(
        "INSERT INTO item_location_events \
            (id, item_id, space_id, from_location_id, to_location_id, actor_account_id) \
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(Uuid::now_v7().to_string())
    .bind(item_id.to_string())
    .bind(space_id.to_string())
    .bind(from_location_id.map(|id| id.to_string()))
    .bind(to_location_id.map(|id| id.to_string()))
    .bind(actor_account_id.to_string())
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn decode_location(row: &sqlx::mysql::MySqlRow) -> Result<Location, LocationError> {
    Ok(Location {
        id: decode_uuid(row, "id")?,
        space_id: decode_uuid(row, "space_id")?,
        parent_id: decode_optional_uuid(row, "parent_id")?,
        name: row.try_get("name")?,
        created_at: row.try_get("created_at")?,
    })
}

fn decode_uuid(row: &sqlx::mysql::MySqlRow, column: &str) -> Result<Uuid, LocationError> {
    let value: Vec<u8> = row.try_get(column)?;
    let value = std::str::from_utf8(&value).map_err(|_| LocationError::MalformedData)?;
    Uuid::parse_str(value).map_err(|_| LocationError::MalformedData)
}

fn decode_optional_uuid(
    row: &sqlx::mysql::MySqlRow,
    column: &str,
) -> Result<Option<Uuid>, LocationError> {
    let value: Option<Vec<u8>> = row.try_get(column)?;
    value
        .map(|value| {
            let value = std::str::from_utf8(&value).map_err(|_| LocationError::MalformedData)?;
            Uuid::parse_str(value).map_err(|_| LocationError::MalformedData)
        })
        .transpose()
}

fn map_duplicate(error: sqlx::Error) -> LocationError {
    if error
        .as_database_error()
        .is_some_and(sqlx::error::DatabaseError::is_unique_violation)
    {
        LocationError::DuplicateName
    } else {
        LocationError::Query(error)
    }
}
