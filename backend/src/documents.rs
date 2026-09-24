//! Space-scoped document metadata and local file storage.
//! A transfer never changes a document's space: the source keeps its own history.

use std::path::{Path, PathBuf};

use chrono::{DateTime, NaiveDateTime, Utc};
use sha2::{Digest, Sha256};
use sqlx::Row;
use tokio::{fs, io::AsyncWriteExt};
use uuid::Uuid;

use crate::Database;

pub const MAX_DOCUMENT_BYTES: usize = 50_000_000;
pub const MAX_DOCUMENTS_PER_ITEM: i64 = 50;

#[derive(Debug, Clone)]
pub struct DocumentStore {
    root: PathBuf,
}

impl DocumentStore {
    /// Creates a private directory if needed. Its canonical path is retained so API input
    /// can never influence the on-disk path.
    ///
    /// # Errors
    ///
    /// Returns an I/O error for an invalid path, failed creation, or unsafe directory mode.
    pub fn new(root: &Path) -> std::io::Result<Self> {
        if !root.is_absolute() {
            return Err(std::io::Error::other(
                "document storage path must be absolute",
            ));
        }
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(root)?;
        let root = root.canonicalize()?;
        if !root.is_dir() {
            return Err(std::io::Error::other("document storage is not a directory"));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = root.metadata()?.permissions().mode();
            if mode & 0o700 != 0o700 || mode & 0o077 != 0 {
                return Err(std::io::Error::other(
                    "document storage directory must have mode 0700",
                ));
            }
        }
        Ok(Self { root })
    }

    fn path(&self, id: Uuid) -> PathBuf {
        self.root.join(id.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    pub id: Uuid,
    pub item_id: Uuid,
    pub space_id: Uuid,
    pub original_name: String,
    pub media_type: String,
    pub byte_size: u64,
    pub sha256: String,
    pub uploaded_by_account_id: Uuid,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug)]
pub enum DocumentError {
    InvalidName,
    InvalidType,
    InvalidSize,
    ItemNotFound,
    WrongSpace,
    ItemTrashed,
    DocumentNotFound,
    QuotaExceeded,
    Storage,
}

fn valid_media_type(bytes: &[u8], media_type: &str) -> bool {
    match media_type {
        "image/jpeg" => bytes.starts_with(&[0xff, 0xd8, 0xff]),
        "image/png" => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
        "image/webp" => bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP",
        "application/pdf" => bytes.starts_with(b"%PDF-"),
        _ => false,
    }
}

fn document_from_row(row: &sqlx::mysql::MySqlRow) -> Result<Document, DocumentError> {
    let text = |key| -> Result<String, DocumentError> {
        // MariaDB/SQLx exposes ascii_bin columns as BINARY/VARBINARY, not String.
        let raw: Vec<u8> = row.try_get(key).map_err(|_| DocumentError::Storage)?;
        String::from_utf8(raw).map_err(|_| DocumentError::Storage)
    };
    let parse = |key| -> Result<Uuid, DocumentError> {
        Uuid::parse_str(&text(key)?).map_err(|_| DocumentError::Storage)
    };
    let created_at: NaiveDateTime = row
        .try_get("created_at")
        .map_err(|_| DocumentError::Storage)?;
    let byte_size: u64 = row
        .try_get("byte_size")
        .map_err(|_| DocumentError::Storage)?;
    Ok(Document {
        id: parse("id")?,
        item_id: parse("item_id")?,
        space_id: parse("space_id")?,
        original_name: row
            .try_get("original_name")
            .map_err(|_| DocumentError::Storage)?,
        media_type: text("media_type")?,
        byte_size,
        sha256: text("sha256")?,
        uploaded_by_account_id: parse("uploaded_by_account_id")?,
        created_at: created_at.and_utc(),
    })
}

const DOCUMENT_COLUMNS: &str = "id, item_id, space_id, original_name, media_type, byte_size, sha256, uploaded_by_account_id, created_at";

impl Database {
    /// Persists the file before inserting its metadata. A failed insert removes the file.
    /// The item row lock serializes uploads, transfers and quota checks.
    ///
    /// # Errors
    ///
    /// Returns validation, item, quota, or storage errors.
    #[allow(clippy::too_many_arguments)]
    pub async fn upload_document(
        &self,
        store: &DocumentStore,
        space_id: Uuid,
        item_id: Uuid,
        actor: Uuid,
        name: &str,
        media_type: &str,
        bytes: &[u8],
    ) -> Result<Document, DocumentError> {
        let name = name.trim();
        if name.is_empty()
            || name.chars().count() > 255
            || name
                .chars()
                .any(|c| c.is_control() || c == '/' || c == '\\')
        {
            return Err(DocumentError::InvalidName);
        }
        if bytes.is_empty() || bytes.len() > MAX_DOCUMENT_BYTES {
            return Err(DocumentError::InvalidSize);
        }
        if !valid_media_type(bytes, media_type) {
            return Err(DocumentError::InvalidType);
        }

        let id = Uuid::now_v7();
        let path = store.path(id);
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(&path).map_err(|_| DocumentError::Storage)?;
        let mut file = fs::File::from_std(file);
        let write_result = async {
            file.write_all(bytes).await?;
            file.sync_all().await
        }
        .await;
        drop(file);
        if write_result.is_err() {
            let _ = fs::remove_file(&path).await;
            return Err(DocumentError::Storage);
        }

        let digest = format!("{:x}", Sha256::digest(bytes));
        let result = async {
            let mut tx = self.pool().begin().await.map_err(|_| DocumentError::Storage)?;
            let row = sqlx::query("SELECT space_id, state FROM inventory_items WHERE id = ? FOR UPDATE")
                .bind(item_id.to_string())
                .fetch_optional(&mut *tx)
                .await
                .map_err(|_| DocumentError::Storage)?
                .ok_or(DocumentError::ItemNotFound)?;
            let current_space: Vec<u8> = row.try_get("space_id").map_err(|_| DocumentError::Storage)?;
            let current_space = std::str::from_utf8(&current_space).map_err(|_| DocumentError::Storage)?;
            if current_space != space_id.to_string() {
                return Err(DocumentError::WrongSpace);
            }
            let state: Vec<u8> = row.try_get("state").map_err(|_| DocumentError::Storage)?;
            let state = std::str::from_utf8(&state).map_err(|_| DocumentError::Storage)?;
            if state == "trashed" {
                return Err(DocumentError::ItemTrashed);
            }
            let count: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM item_documents WHERE item_id = ? AND space_id = ? AND deleted_at IS NULL",
            )
            .bind(item_id.to_string())
            .bind(space_id.to_string())
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| DocumentError::Storage)?;
            if count >= MAX_DOCUMENTS_PER_ITEM {
                return Err(DocumentError::QuotaExceeded);
            }
            sqlx::query(
                "INSERT INTO item_documents (id, item_id, space_id, original_name, media_type, byte_size, sha256, uploaded_by_account_id) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(id.to_string())
            .bind(item_id.to_string())
            .bind(space_id.to_string())
            .bind(name)
            .bind(media_type)
            .bind(u64::try_from(bytes.len()).map_err(|_| DocumentError::InvalidSize)?)
            .bind(&digest)
            .bind(actor.to_string())
            .execute(&mut *tx)
            .await
            .map_err(|_| DocumentError::Storage)?;
            tx.commit().await.map_err(|_| DocumentError::Storage)?;
            self.document(space_id, item_id, id).await
        }
        .await;
        if result.is_err() {
            // A committed row is intentionally retained if the final read fails; deleting its
            // file would make the document permanently unreadable.
            let committed =
                sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM item_documents WHERE id = ?")
                    .bind(id.to_string())
                    .fetch_one(self.pool())
                    .await
                    .unwrap_or(1);
            if committed == 0 {
                let _ = fs::remove_file(&path).await;
            }
        }
        result
    }

    /// Lists active documents retained by this space, including after an item transfer.
    ///
    /// # Errors
    ///
    /// Returns a storage error if MariaDB cannot be read.
    pub async fn documents(
        &self,
        space_id: Uuid,
        item_id: Uuid,
    ) -> Result<Vec<Document>, DocumentError> {
        let query = format!(
            "SELECT {DOCUMENT_COLUMNS} FROM item_documents WHERE space_id = ? AND item_id = ? AND deleted_at IS NULL ORDER BY created_at, id"
        );
        let rows = sqlx::query(&query)
            .bind(space_id.to_string())
            .bind(item_id.to_string())
            .fetch_all(self.pool())
            .await
            .map_err(|_| DocumentError::Storage)?;
        rows.iter().map(document_from_row).collect()
    }

    /// Reads one active document's metadata within its owning space.
    ///
    /// # Errors
    ///
    /// Returns not found or storage error.
    pub async fn document(
        &self,
        space_id: Uuid,
        item_id: Uuid,
        id: Uuid,
    ) -> Result<Document, DocumentError> {
        let query = format!(
            "SELECT {DOCUMENT_COLUMNS} FROM item_documents WHERE id = ? AND space_id = ? AND item_id = ? AND deleted_at IS NULL"
        );
        let row = sqlx::query(&query)
            .bind(id.to_string())
            .bind(space_id.to_string())
            .bind(item_id.to_string())
            .fetch_optional(self.pool())
            .await
            .map_err(|_| DocumentError::Storage)?
            .ok_or(DocumentError::DocumentNotFound)?;
        document_from_row(&row)
    }

    /// Reads and checks a stored file against its recorded size and digest.
    ///
    /// # Errors
    ///
    /// Returns a storage error if the file is missing or inconsistent.
    pub async fn document_bytes(
        &self,
        store: &DocumentStore,
        document: &Document,
    ) -> Result<Vec<u8>, DocumentError> {
        let bytes = fs::read(store.path(document.id))
            .await
            .map_err(|_| DocumentError::Storage)?;
        if bytes.len() as u64 != document.byte_size
            || format!("{:x}", Sha256::digest(&bytes)) != document.sha256
        {
            return Err(DocumentError::Storage);
        }
        Ok(bytes)
    }

    /// Soft deletion retains the file and metadata until a retention policy is approved.
    ///
    /// # Errors
    ///
    /// Returns not found or storage error.
    pub async fn delete_document(
        &self,
        space_id: Uuid,
        item_id: Uuid,
        id: Uuid,
    ) -> Result<(), DocumentError> {
        let affected = sqlx::query(
            "UPDATE item_documents SET deleted_at = CURRENT_TIMESTAMP(6) WHERE id = ? AND space_id = ? AND item_id = ? AND deleted_at IS NULL",
        )
        .bind(id.to_string())
        .bind(space_id.to_string())
        .bind(item_id.to_string())
        .execute(self.pool())
        .await
        .map_err(|_| DocumentError::Storage)?
        .rows_affected();
        if affected == 0 {
            return Err(DocumentError::DocumentNotFound);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::valid_media_type;

    #[test]
    fn sniffing_rejects_mismatched_or_unsupported_media() {
        assert!(valid_media_type(b"%PDF-1.7\n", "application/pdf"));
        assert!(valid_media_type(b"\x89PNG\r\n\x1a\n", "image/png"));
        assert!(!valid_media_type(b"%PDF-1.7\n", "image/png"));
        assert!(!valid_media_type(b"MZ", "application/pdf"));
        assert!(!valid_media_type(b"hello", "text/plain"));
    }
}
