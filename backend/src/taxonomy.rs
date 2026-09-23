//! Space-scoped, nested inventory categories and their field definitions.

use std::collections::BTreeSet;

use chrono::{DateTime, NaiveDate, Utc};
use sqlx::Row;
use uuid::Uuid;

use crate::database::{Database, DatabaseError, InventoryError};

const ROOT_PARENT_KEY: &str = "00000000-0000-0000-0000-000000000000";
const MAX_LABEL_CHARS: usize = 255;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Category {
    pub id: Uuid,
    pub space_id: Uuid,
    pub parent_id: Option<Uuid>,
    pub name: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldType {
    Text,
    Number,
    Date,
}

impl FieldType {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Number => "number",
            Self::Date => "date",
        }
    }

    #[must_use]
    pub fn from_code(code: &str) -> Option<Self> {
        match code {
            "text" => Some(Self::Text),
            "number" => Some(Self::Number),
            "date" => Some(Self::Date),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CategoryField {
    pub id: Uuid,
    pub category_id: Uuid,
    pub name: String,
    pub value_type: FieldType,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemCategory {
    pub assignment_id: Uuid,
    pub category_id: Uuid,
    pub category_name: String,
    pub assigned_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveField {
    pub id: Uuid,
    pub assignment_id: Uuid,
    pub defined_category_id: Uuid,
    pub defined_category_name: String,
    pub name: String,
    pub value_type: FieldType,
    pub value: Option<String>,
}

#[derive(Debug)]
pub enum TaxonomyError {
    Query(sqlx::Error),
    InvalidName,
    SpaceNotFound,
    ParentNotFound,
    CategoryNotFound,
    DuplicateName,
    InvalidFieldType,
    ItemNotFound,
    RevisionConflict,
    ItemInTrash,
    EmptySelection,
    TooManyCategories,
    InvalidCategory,
    FieldNotAvailable,
    InvalidValue,
    MalformedData,
}

impl From<sqlx::Error> for TaxonomyError {
    fn from(error: sqlx::Error) -> Self {
        Self::Query(error)
    }
}

impl Database {
    /// Creates a root category or a child of a category in the same space.
    ///
    /// # Errors
    ///
    /// Returns a validation, missing parent/space, duplicate sibling, or query error.
    pub async fn create_category(
        &self,
        space_id: Uuid,
        parent_id: Option<Uuid>,
        name: &str,
    ) -> Result<Category, TaxonomyError> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > MAX_LABEL_CHARS {
            return Err(TaxonomyError::InvalidName);
        }

        let mut tx = self.pool().begin().await?;
        // A space lock serializes sibling-name checks, including the NULL-parent root case.
        let space: Option<Vec<u8>> =
            sqlx::query_scalar("SELECT id FROM spaces WHERE id = ? FOR UPDATE")
                .bind(space_id.to_string())
                .fetch_optional(&mut *tx)
                .await?;
        if space.is_none() {
            return Err(TaxonomyError::SpaceNotFound);
        }
        if let Some(parent_id) = parent_id {
            let parent: Option<Vec<u8>> = sqlx::query_scalar(
                "SELECT id FROM inventory_categories WHERE id = ? AND space_id = ?",
            )
            .bind(parent_id.to_string())
            .bind(space_id.to_string())
            .fetch_optional(&mut *tx)
            .await?;
            if parent.is_none() {
                return Err(TaxonomyError::ParentNotFound);
            }
        }
        let parent_key = parent_id.map_or_else(|| ROOT_PARENT_KEY.to_owned(), |id| id.to_string());
        let duplicate: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM inventory_categories \
             WHERE space_id = ? AND parent_key = ? AND name = ?",
        )
        .bind(space_id.to_string())
        .bind(&parent_key)
        .bind(name)
        .fetch_one(&mut *tx)
        .await?;
        if duplicate != 0 {
            return Err(TaxonomyError::DuplicateName);
        }

        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO inventory_categories (id, space_id, parent_id, parent_key, name) \
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
        self.category(space_id, id).await
    }

    /// Lists every category in an authorized space; the client may build the hierarchy from
    /// stable parent identifiers without relying on presentation order.
    ///
    /// # Errors
    ///
    /// Returns a query or malformed-data error.
    pub async fn categories_in_space(
        &self,
        space_id: Uuid,
    ) -> Result<Vec<Category>, TaxonomyError> {
        let rows = sqlx::query(
            "SELECT id, space_id, parent_id, name, created_at \
             FROM inventory_categories WHERE space_id = ? ORDER BY name, id",
        )
        .bind(space_id.to_string())
        .fetch_all(self.pool())
        .await?;
        rows.iter().map(decode_category).collect()
    }

    /// Loads one category only if it belongs to the specified space.
    ///
    /// # Errors
    ///
    /// Returns `CategoryNotFound`, a query error, or malformed data.
    pub async fn category(&self, space_id: Uuid, id: Uuid) -> Result<Category, TaxonomyError> {
        let row = sqlx::query(
            "SELECT id, space_id, parent_id, name, created_at \
             FROM inventory_categories WHERE id = ? AND space_id = ?",
        )
        .bind(id.to_string())
        .bind(space_id.to_string())
        .fetch_optional(self.pool())
        .await?
        .ok_or(TaxonomyError::CategoryNotFound)?;
        decode_category(&row)
    }

    /// Creates a text, number or date field owned by a category.
    ///
    /// # Errors
    ///
    /// Returns a validation, missing category, duplicate name, or query error.
    pub async fn create_category_field(
        &self,
        space_id: Uuid,
        category_id: Uuid,
        name: &str,
        value_type: FieldType,
    ) -> Result<CategoryField, TaxonomyError> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > MAX_LABEL_CHARS {
            return Err(TaxonomyError::InvalidName);
        }
        let mut tx = self.pool().begin().await?;
        let category: Option<Vec<u8>> = sqlx::query_scalar(
            "SELECT id FROM inventory_categories WHERE id = ? AND space_id = ? FOR UPDATE",
        )
        .bind(category_id.to_string())
        .bind(space_id.to_string())
        .fetch_optional(&mut *tx)
        .await?;
        if category.is_none() {
            return Err(TaxonomyError::CategoryNotFound);
        }
        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO category_field_definitions (id, category_id, name, value_type) \
             VALUES (?, ?, ?, ?)",
        )
        .bind(id.to_string())
        .bind(category_id.to_string())
        .bind(name)
        .bind(value_type.as_str())
        .execute(&mut *tx)
        .await
        .map_err(map_duplicate)?;
        tx.commit().await?;
        self.category_field(id).await
    }

    /// Lists fields defined directly on a category. Effective inherited fields are calculated
    /// for the item later, so this endpoint does not silently duplicate parent definitions.
    ///
    /// # Errors
    ///
    /// Returns a missing-category, query, or malformed-data error.
    pub async fn category_fields(
        &self,
        space_id: Uuid,
        category_id: Uuid,
    ) -> Result<Vec<CategoryField>, TaxonomyError> {
        self.category(space_id, category_id).await?;
        let rows = sqlx::query(
            "SELECT id, category_id, name, value_type, created_at \
             FROM category_field_definitions WHERE category_id = ? ORDER BY name, id",
        )
        .bind(category_id.to_string())
        .fetch_all(self.pool())
        .await?;
        rows.iter().map(decode_field).collect()
    }

    async fn category_field(&self, id: Uuid) -> Result<CategoryField, TaxonomyError> {
        let row = sqlx::query(
            "SELECT id, category_id, name, value_type, created_at \
             FROM category_field_definitions WHERE id = ?",
        )
        .bind(id.to_string())
        .fetch_optional(self.pool())
        .await?
        .ok_or(TaxonomyError::MalformedData)?;
        decode_field(&row)
    }

    /// Adds one or more classifications to an item without discarding existing assignments.
    /// The item row is locked and its revision is checked before any category is attached.
    ///
    /// # Errors
    ///
    /// Returns a validation, missing item/category, stale revision, trash, or query error.
    pub async fn add_item_categories(
        &self,
        item_id: Uuid,
        expected_revision: u64,
        category_ids: &[Uuid],
    ) -> Result<u64, TaxonomyError> {
        let selected: BTreeSet<Uuid> = category_ids.iter().copied().collect();
        if selected.is_empty() {
            return Err(TaxonomyError::EmptySelection);
        }
        if selected.len() > 20 {
            return Err(TaxonomyError::TooManyCategories);
        }
        let mut tx = self.pool().begin().await?;
        let row = sqlx::query(
            "SELECT space_id, state, revision FROM inventory_items WHERE id = ? FOR UPDATE",
        )
        .bind(item_id.to_string())
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(TaxonomyError::ItemNotFound)?;
        let space_id = decode_uuid(&row, "space_id")?;
        let state: Vec<u8> = row.try_get("state")?;
        if state == b"trashed" {
            return Err(TaxonomyError::ItemInTrash);
        }
        let revision: u64 = row.try_get("revision")?;
        if revision != expected_revision {
            return Err(TaxonomyError::RevisionConflict);
        }

        let existing_rows = sqlx::query(
            "SELECT category_id FROM item_category_assignments \
             WHERE item_id = ? AND space_id = ? AND ended_at IS NULL",
        )
        .bind(item_id.to_string())
        .bind(space_id.to_string())
        .fetch_all(&mut *tx)
        .await?;
        let existing: BTreeSet<Uuid> = existing_rows
            .iter()
            .map(|row| decode_uuid(row, "category_id"))
            .collect::<Result<_, _>>()?;
        if existing.len() + selected.difference(&existing).count() > 20 {
            return Err(TaxonomyError::TooManyCategories);
        }
        let mut added = false;
        for category_id in selected.difference(&existing) {
            let belongs: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM inventory_categories WHERE id = ? AND space_id = ?",
            )
            .bind(category_id.to_string())
            .bind(space_id.to_string())
            .fetch_one(&mut *tx)
            .await?;
            if belongs == 0 {
                return Err(TaxonomyError::InvalidCategory);
            }
            sqlx::query(
                "INSERT INTO item_category_assignments (id, item_id, category_id, space_id) \
                 VALUES (?, ?, ?, ?)",
            )
            .bind(Uuid::now_v7().to_string())
            .bind(item_id.to_string())
            .bind(category_id.to_string())
            .bind(space_id.to_string())
            .execute(&mut *tx)
            .await?;
            added = true;
        }
        if added {
            sqlx::query("UPDATE inventory_items SET revision = revision + 1 WHERE id = ?")
                .bind(item_id.to_string())
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(revision + u64::from(added))
    }

    /// Lists an item's current categories in a single space.
    ///
    /// # Errors
    ///
    /// Returns a query or malformed-data error.
    pub async fn item_categories(
        &self,
        item_id: Uuid,
        space_id: Uuid,
    ) -> Result<Vec<ItemCategory>, TaxonomyError> {
        let rows = sqlx::query(
            "SELECT a.id AS assignment_id, a.category_id, c.name AS category_name, a.assigned_at \
             FROM item_category_assignments a \
             JOIN inventory_categories c ON c.id = a.category_id \
             WHERE a.item_id = ? AND a.space_id = ? AND a.ended_at IS NULL \
             ORDER BY c.name, a.id",
        )
        .bind(item_id.to_string())
        .bind(space_id.to_string())
        .fetch_all(self.pool())
        .await?;
        rows.iter()
            .map(|row| {
                Ok(ItemCategory {
                    assignment_id: decode_uuid(row, "assignment_id")?,
                    category_id: decode_uuid(row, "category_id")?,
                    category_name: row.try_get("category_name")?,
                    assigned_at: row.try_get("assigned_at")?,
                })
            })
            .collect()
    }

    /// Lists fields from every directly assigned category and its ancestors. If two assigned
    /// branches share an ancestor, its field appears once for the item.
    ///
    /// # Errors
    ///
    /// Returns a query or malformed-data error.
    pub async fn effective_item_fields(
        &self,
        item_id: Uuid,
        space_id: Uuid,
    ) -> Result<Vec<EffectiveField>, TaxonomyError> {
        load_effective_fields(self.pool(), item_id, space_id).await
    }

    /// Sets a field value with optimistic concurrency. The field must be available through at
    /// least one active category assignment or an ancestor of one.
    ///
    /// # Errors
    ///
    /// Returns a validation, missing item/field, stale revision, trash, or query error.
    pub async fn set_item_field_value(
        &self,
        item_id: Uuid,
        field_id: Uuid,
        expected_revision: u64,
        value: &str,
    ) -> Result<u64, TaxonomyError> {
        let mut tx = self.pool().begin().await?;
        let row = sqlx::query(
            "SELECT space_id, state, revision FROM inventory_items WHERE id = ? FOR UPDATE",
        )
        .bind(item_id.to_string())
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(TaxonomyError::ItemNotFound)?;
        let space_id = decode_uuid(&row, "space_id")?;
        let state: Vec<u8> = row.try_get("state")?;
        if state == b"trashed" {
            return Err(TaxonomyError::ItemInTrash);
        }
        let revision: u64 = row.try_get("revision")?;
        if revision != expected_revision {
            return Err(TaxonomyError::RevisionConflict);
        }
        let fields = load_effective_fields(&mut *tx, item_id, space_id).await?;
        let field = fields
            .iter()
            .find(|field| field.id == field_id)
            .ok_or(TaxonomyError::FieldNotAvailable)?;
        let (text_value, number_value, date_value) = match field.value_type {
            FieldType::Text if value.chars().count() <= 4096 => (Some(value), None, None),
            FieldType::Number if valid_decimal(value) => (None, Some(value), None),
            FieldType::Date if NaiveDate::parse_from_str(value, "%Y-%m-%d").is_ok() => {
                (None, None, Some(value))
            }
            _ => return Err(TaxonomyError::InvalidValue),
        };
        sqlx::query(
            "INSERT INTO item_custom_field_values \
                (assignment_id, field_id, value_text, value_number, value_date) \
             VALUES (?, ?, ?, ?, ?) \
             ON DUPLICATE KEY UPDATE value_text = VALUES(value_text), \
                value_number = VALUES(value_number), value_date = VALUES(value_date), \
                updated_at = CURRENT_TIMESTAMP(6)",
        )
        .bind(field.assignment_id.to_string())
        .bind(field_id.to_string())
        .bind(text_value)
        .bind(number_value)
        .bind(date_value)
        .execute(&mut *tx)
        .await?;
        sqlx::query("UPDATE inventory_items SET revision = revision + 1 WHERE id = ?")
            .bind(item_id.to_string())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(revision + 1)
    }
}

async fn load_effective_fields<'e, E>(
    executor: E,
    item_id: Uuid,
    space_id: Uuid,
) -> Result<Vec<EffectiveField>, TaxonomyError>
where
    E: sqlx::Executor<'e, Database = sqlx::MySql>,
{
    let rows = sqlx::query(
        "WITH RECURSIVE ancestry AS ( \
             SELECT a.id AS assignment_id, a.assigned_at, c.id AS category_id, c.parent_id \
             FROM item_category_assignments a \
             JOIN inventory_categories c ON c.id = a.category_id \
             WHERE a.item_id = ? AND a.space_id = ? AND a.ended_at IS NULL \
             UNION ALL \
             SELECT ancestry.assignment_id, ancestry.assigned_at, p.id, p.parent_id \
             FROM ancestry JOIN inventory_categories p ON p.id = ancestry.parent_id \
         ) \
         SELECT ancestry.assignment_id, f.id AS field_id, f.category_id AS defined_category_id, \
                c.name AS defined_category_name, f.name, f.value_type, \
                COALESCE(v.value_text, CAST(v.value_number AS CHAR), \
                    DATE_FORMAT(v.value_date, '%Y-%m-%d')) AS field_value \
         FROM ancestry \
         JOIN category_field_definitions f ON f.category_id = ancestry.category_id \
         JOIN inventory_categories c ON c.id = f.category_id \
         LEFT JOIN item_custom_field_values v \
           ON v.assignment_id = ancestry.assignment_id AND v.field_id = f.id \
         ORDER BY f.id, (v.assignment_id IS NULL), ancestry.assigned_at, ancestry.assignment_id",
    )
    .bind(item_id.to_string())
    .bind(space_id.to_string())
    .fetch_all(executor)
    .await?;
    let mut seen = BTreeSet::new();
    let mut fields = Vec::new();
    for row in &rows {
        let id = decode_uuid(row, "field_id")?;
        if !seen.insert(id) {
            continue;
        }
        let code: Vec<u8> = row.try_get("value_type")?;
        let code = std::str::from_utf8(&code).map_err(|_| TaxonomyError::MalformedData)?;
        fields.push(EffectiveField {
            id,
            assignment_id: decode_uuid(row, "assignment_id")?,
            defined_category_id: decode_uuid(row, "defined_category_id")?,
            defined_category_name: row.try_get("defined_category_name")?,
            name: row.try_get("name")?,
            value_type: FieldType::from_code(code).ok_or(TaxonomyError::InvalidFieldType)?,
            value: row.try_get("field_value")?,
        });
    }
    Ok(fields)
}

fn valid_decimal(value: &str) -> bool {
    let unsigned = value.strip_prefix('-').unwrap_or(value);
    let decimal = unsigned.split_once('.');
    let (integer, fractional) = decimal.unwrap_or((unsigned, ""));
    !integer.is_empty()
        && integer.len() <= 14
        && integer.bytes().all(|c| c.is_ascii_digit())
        && fractional.len() <= 6
        && (decimal.is_none() || !fractional.is_empty())
        && fractional.bytes().all(|c| c.is_ascii_digit())
        && !value.contains('+')
        && value.matches('.').count() <= 1
}

/// Ends source-space classifications and starts explicitly selected destination ones as part of
/// the caller's transfer transaction. Historical values remain attached to ended assignments.
pub(crate) async fn apply_transfer_categories(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    item_id: Uuid,
    source_space_id: Uuid,
    destination_space_id: Uuid,
    destination_category_ids: &[Uuid],
) -> Result<(), DatabaseError> {
    let selected: BTreeSet<Uuid> = destination_category_ids.iter().copied().collect();
    if selected.len() > 20 {
        return Err(DatabaseError::Inventory(
            InventoryError::InvalidDestinationCategory,
        ));
    }
    let current_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM item_category_assignments \
         WHERE item_id = ? AND space_id = ? AND ended_at IS NULL",
    )
    .bind(item_id.to_string())
    .bind(source_space_id.to_string())
    .fetch_one(&mut **tx)
    .await
    .map_err(DatabaseError::Query)?;
    if current_count > 0 && selected.is_empty() {
        return Err(DatabaseError::Inventory(
            InventoryError::DestinationCategoriesRequired,
        ));
    }
    for category_id in &selected {
        let belongs: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM inventory_categories WHERE id = ? AND space_id = ?",
        )
        .bind(category_id.to_string())
        .bind(destination_space_id.to_string())
        .fetch_one(&mut **tx)
        .await
        .map_err(DatabaseError::Query)?;
        if belongs == 0 {
            return Err(DatabaseError::Inventory(
                InventoryError::InvalidDestinationCategory,
            ));
        }
    }
    sqlx::query(
        "UPDATE item_category_assignments SET ended_at = ? \
         WHERE item_id = ? AND space_id = ? AND ended_at IS NULL",
    )
    .bind(Utc::now())
    .bind(item_id.to_string())
    .bind(source_space_id.to_string())
    .execute(&mut **tx)
    .await
    .map_err(DatabaseError::Query)?;
    for category_id in selected {
        sqlx::query(
            "INSERT INTO item_category_assignments (id, item_id, category_id, space_id) \
             VALUES (?, ?, ?, ?)",
        )
        .bind(Uuid::now_v7().to_string())
        .bind(item_id.to_string())
        .bind(category_id.to_string())
        .bind(destination_space_id.to_string())
        .execute(&mut **tx)
        .await
        .map_err(DatabaseError::Query)?;
    }
    Ok(())
}

fn decode_category(row: &sqlx::mysql::MySqlRow) -> Result<Category, TaxonomyError> {
    Ok(Category {
        id: decode_uuid(row, "id")?,
        space_id: decode_uuid(row, "space_id")?,
        parent_id: decode_optional_uuid(row, "parent_id")?,
        name: row.try_get("name")?,
        created_at: row.try_get("created_at")?,
    })
}

fn decode_field(row: &sqlx::mysql::MySqlRow) -> Result<CategoryField, TaxonomyError> {
    let code: Vec<u8> = row.try_get("value_type")?;
    let code = std::str::from_utf8(&code).map_err(|_| TaxonomyError::MalformedData)?;
    Ok(CategoryField {
        id: decode_uuid(row, "id")?,
        category_id: decode_uuid(row, "category_id")?,
        name: row.try_get("name")?,
        value_type: FieldType::from_code(code).ok_or(TaxonomyError::InvalidFieldType)?,
        created_at: row.try_get("created_at")?,
    })
}

fn decode_uuid(row: &sqlx::mysql::MySqlRow, column: &str) -> Result<Uuid, TaxonomyError> {
    let value: Vec<u8> = row.try_get(column)?;
    let value = std::str::from_utf8(&value).map_err(|_| TaxonomyError::MalformedData)?;
    Uuid::parse_str(value).map_err(|_| TaxonomyError::MalformedData)
}

fn decode_optional_uuid(
    row: &sqlx::mysql::MySqlRow,
    column: &str,
) -> Result<Option<Uuid>, TaxonomyError> {
    let value: Option<Vec<u8>> = row.try_get(column)?;
    value
        .map(|value| {
            let value = std::str::from_utf8(&value).map_err(|_| TaxonomyError::MalformedData)?;
            Uuid::parse_str(value).map_err(|_| TaxonomyError::MalformedData)
        })
        .transpose()
}

fn map_duplicate(error: sqlx::Error) -> TaxonomyError {
    if error
        .as_database_error()
        .is_some_and(sqlx::error::DatabaseError::is_unique_violation)
    {
        TaxonomyError::DuplicateName
    } else {
        TaxonomyError::Query(error)
    }
}
