//! Non-financial acquisition prospects: wishes, vendors and offers.

use chrono::{DateTime, Utc};
use sqlx::Row;
use url::Url;
use uuid::Uuid;

use crate::database::Database;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wish {
    pub id: Uuid,
    pub space_id: Uuid,
    pub title: String,
    pub search_notes: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vendor {
    pub id: Uuid,
    pub space_id: Uuid,
    pub name: String,
    pub website_url: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    pub id: Uuid,
    pub space_id: Uuid,
    pub wish_id: Uuid,
    pub vendor_id: Uuid,
    pub title: String,
    pub source_url: Option<String>,
    pub notes: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug)]
pub enum AcquisitionError {
    Query(sqlx::Error),
    InvalidTitle,
    InvalidNotes,
    InvalidUrl,
    DuplicateVendor,
    WishNotFound,
    VendorNotFound,
    MalformedData,
}

impl From<sqlx::Error> for AcquisitionError {
    fn from(error: sqlx::Error) -> Self {
        Self::Query(error)
    }
}

impl Database {
    /// Creates an entry in a space's wish list.
    ///
    /// # Errors
    ///
    /// Returns validation or storage errors.
    pub async fn create_wish(
        &self,
        space_id: Uuid,
        title: &str,
        search_notes: Option<&str>,
    ) -> Result<Wish, AcquisitionError> {
        let title = normalize_title(title)?;
        let search_notes = normalize_notes(search_notes)?;
        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO wishlist_entries (id, space_id, title, search_notes) VALUES (?, ?, ?, ?)",
        )
        .bind(id.to_string())
        .bind(space_id.to_string())
        .bind(title)
        .bind(search_notes)
        .execute(self.pool())
        .await?;
        self.wish(space_id, id).await
    }

    /// Lists a space's wishes in creation order.
    ///
    /// # Errors
    ///
    /// Returns a storage or malformed-data error.
    pub async fn wishes_in_space(&self, space_id: Uuid) -> Result<Vec<Wish>, AcquisitionError> {
        let rows = sqlx::query("SELECT id, space_id, title, search_notes, created_at FROM wishlist_entries WHERE space_id = ? ORDER BY created_at, id")
            .bind(space_id.to_string()).fetch_all(self.pool()).await?;
        rows.iter().map(decode_wish).collect()
    }

    /// Loads a wish scoped to its space.
    ///
    /// # Errors
    ///
    /// Returns not-found, storage, or malformed-data errors.
    pub async fn wish(&self, space_id: Uuid, wish_id: Uuid) -> Result<Wish, AcquisitionError> {
        let row = sqlx::query("SELECT id, space_id, title, search_notes, created_at FROM wishlist_entries WHERE space_id = ? AND id = ?")
            .bind(space_id.to_string()).bind(wish_id.to_string())
            .fetch_optional(self.pool()).await?.ok_or(AcquisitionError::WishNotFound)?;
        decode_wish(&row)
    }

    /// Creates a vendor scoped to one space.
    ///
    /// # Errors
    ///
    /// Returns validation, duplicate, or storage errors.
    pub async fn create_vendor(
        &self,
        space_id: Uuid,
        name: &str,
        website_url: Option<&str>,
    ) -> Result<Vendor, AcquisitionError> {
        let name = normalize_title(name)?;
        let website_url = normalize_url(website_url)?;
        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO acquisition_vendors (id, space_id, name, website_url) VALUES (?, ?, ?, ?)",
        )
        .bind(id.to_string())
        .bind(space_id.to_string())
        .bind(name)
        .bind(website_url)
        .execute(self.pool())
        .await
        .map_err(|error| {
            if matches!(&error, sqlx::Error::Database(db) if db.is_unique_violation()) {
                AcquisitionError::DuplicateVendor
            } else {
                AcquisitionError::Query(error)
            }
        })?;
        self.vendor(space_id, id).await
    }

    /// Lists the vendors known in one space.
    ///
    /// # Errors
    ///
    /// Returns a storage or malformed-data error.
    pub async fn vendors_in_space(&self, space_id: Uuid) -> Result<Vec<Vendor>, AcquisitionError> {
        let rows = sqlx::query("SELECT id, space_id, name, website_url, created_at FROM acquisition_vendors WHERE space_id = ? ORDER BY name, id")
            .bind(space_id.to_string()).fetch_all(self.pool()).await?;
        rows.iter().map(decode_vendor).collect()
    }

    /// Loads a vendor scoped to its space.
    ///
    /// # Errors
    ///
    /// Returns not-found, storage, or malformed-data errors.
    pub async fn vendor(
        &self,
        space_id: Uuid,
        vendor_id: Uuid,
    ) -> Result<Vendor, AcquisitionError> {
        let row = sqlx::query("SELECT id, space_id, name, website_url, created_at FROM acquisition_vendors WHERE space_id = ? AND id = ?")
            .bind(space_id.to_string()).bind(vendor_id.to_string())
            .fetch_optional(self.pool()).await?.ok_or(AcquisitionError::VendorNotFound)?;
        decode_vendor(&row)
    }

    /// Records an offer for a wish and vendor in the same space, without a price.
    ///
    /// # Errors
    ///
    /// Returns validation, missing wish/vendor, or storage errors.
    pub async fn create_offer(
        &self,
        space_id: Uuid,
        wish_id: Uuid,
        vendor_id: Uuid,
        title: &str,
        source_url: Option<&str>,
        notes: Option<&str>,
    ) -> Result<Offer, AcquisitionError> {
        let title = normalize_title(title)?;
        let source_url = normalize_url(source_url)?;
        let notes = normalize_notes(notes)?;
        self.wish(space_id, wish_id).await?;
        self.vendor(space_id, vendor_id).await?;
        let id = Uuid::now_v7();
        sqlx::query("INSERT INTO acquisition_offers (id, space_id, wish_id, vendor_id, title, source_url, notes) VALUES (?, ?, ?, ?, ?, ?, ?)")
            .bind(id.to_string()).bind(space_id.to_string()).bind(wish_id.to_string())
            .bind(vendor_id.to_string()).bind(title).bind(source_url).bind(notes)
            .execute(self.pool()).await?;
        self.offer(space_id, wish_id, id).await
    }

    /// Lists all offers for a wish in its space.
    ///
    /// # Errors
    ///
    /// Returns not-found, storage, or malformed-data errors.
    pub async fn offers_for_wish(
        &self,
        space_id: Uuid,
        wish_id: Uuid,
    ) -> Result<Vec<Offer>, AcquisitionError> {
        self.wish(space_id, wish_id).await?;
        let rows = sqlx::query("SELECT id, space_id, wish_id, vendor_id, title, source_url, notes, created_at FROM acquisition_offers WHERE space_id = ? AND wish_id = ? ORDER BY created_at, id")
            .bind(space_id.to_string()).bind(wish_id.to_string()).fetch_all(self.pool()).await?;
        rows.iter().map(decode_offer).collect()
    }

    /// Loads one offer attached to a wish in the same space.
    ///
    /// # Errors
    ///
    /// Returns not-found, storage, or malformed-data errors.
    pub async fn offer(
        &self,
        space_id: Uuid,
        wish_id: Uuid,
        offer_id: Uuid,
    ) -> Result<Offer, AcquisitionError> {
        let row = sqlx::query("SELECT id, space_id, wish_id, vendor_id, title, source_url, notes, created_at FROM acquisition_offers WHERE space_id = ? AND wish_id = ? AND id = ?")
            .bind(space_id.to_string()).bind(wish_id.to_string()).bind(offer_id.to_string())
            .fetch_optional(self.pool()).await?.ok_or(AcquisitionError::WishNotFound)?;
        decode_offer(&row)
    }
}

fn normalize_title(value: &str) -> Result<&str, AcquisitionError> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > 255 {
        return Err(AcquisitionError::InvalidTitle);
    }
    Ok(value)
}

fn normalize_notes(value: Option<&str>) -> Result<Option<String>, AcquisitionError> {
    let value = value.map(str::trim).filter(|value| !value.is_empty());
    if value.is_some_and(|value| value.chars().count() > 10_000) {
        return Err(AcquisitionError::InvalidNotes);
    }
    Ok(value.map(str::to_owned))
}

fn normalize_url(value: Option<&str>) -> Result<Option<String>, AcquisitionError> {
    let value = value.map(str::trim).filter(|value| !value.is_empty());
    let Some(value) = value else {
        return Ok(None);
    };
    if value.len() > 2048 {
        return Err(AcquisitionError::InvalidUrl);
    }
    let parsed = Url::parse(value).map_err(|_| AcquisitionError::InvalidUrl)?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err(AcquisitionError::InvalidUrl);
    }
    Ok(Some(value.to_owned()))
}

fn decode_wish(row: &sqlx::mysql::MySqlRow) -> Result<Wish, AcquisitionError> {
    Ok(Wish {
        id: decode_uuid(row, "id")?,
        space_id: decode_uuid(row, "space_id")?,
        title: row.try_get("title")?,
        search_notes: row.try_get("search_notes")?,
        created_at: row.try_get("created_at")?,
    })
}

fn decode_vendor(row: &sqlx::mysql::MySqlRow) -> Result<Vendor, AcquisitionError> {
    Ok(Vendor {
        id: decode_uuid(row, "id")?,
        space_id: decode_uuid(row, "space_id")?,
        name: row.try_get("name")?,
        website_url: row.try_get("website_url")?,
        created_at: row.try_get("created_at")?,
    })
}

fn decode_offer(row: &sqlx::mysql::MySqlRow) -> Result<Offer, AcquisitionError> {
    Ok(Offer {
        id: decode_uuid(row, "id")?,
        space_id: decode_uuid(row, "space_id")?,
        wish_id: decode_uuid(row, "wish_id")?,
        vendor_id: decode_uuid(row, "vendor_id")?,
        title: row.try_get("title")?,
        source_url: row.try_get("source_url")?,
        notes: row.try_get("notes")?,
        created_at: row.try_get("created_at")?,
    })
}

fn decode_uuid(row: &sqlx::mysql::MySqlRow, column: &str) -> Result<Uuid, AcquisitionError> {
    let bytes: Vec<u8> = row.try_get(column)?;
    let value = std::str::from_utf8(&bytes).map_err(|_| AcquisitionError::MalformedData)?;
    Uuid::parse_str(value).map_err(|_| AcquisitionError::MalformedData)
}
