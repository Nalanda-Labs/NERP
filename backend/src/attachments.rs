//! Attachments in the office editor: a Word, Excel or PowerPoint file attached to a task, a
//! chat message or a feed post opens for co-editing like a drive file.
//!
//! - **Who:** anyone who may see the file reads it; editing is for a task's creator,
//!   responsible person and co-workers, a chat's members, and the people a post is for.
//! - **Saving** updates the file in place — every task, message and post showing it shows the
//!   new content — and keeps the earlier contents (`file_versions`).
use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::{auth::AuthUser, drive::OfficeFile, error::{AppError, AppResult}, state::AppState};

/// Whether the person may change an attachment (beyond seeing it).
pub async fn can_edit(state: &AppState, auth: &AuthUser, file: Uuid) -> AppResult<bool> {
    if auth.is_guest() {
        return Ok(false);
    }
    let task: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM task_files f JOIN tasks t ON t.id = f.task_id WHERE f.file_id = $1
                         AND (t.created_by = $2 OR t.responsible_id = $2 OR EXISTS (SELECT 1 FROM task_members m WHERE m.task_id = t.id AND m.user_id = $2 AND m.role = 'assistant')))",
    )
    .bind(file)
    .bind(auth.id)
    .fetch_one(&state.db)
    .await?;
    if task {
        return Ok(true);
    }
    let chat: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM im_messages m JOIN im_members mm ON mm.chat_id = m.chat_id AND mm.user_id = $2 WHERE $1 = ANY(m.file_ids) AND m.deleted_at IS NULL)",
    )
    .bind(file)
    .bind(auth.id)
    .fetch_one(&state.db)
    .await?;
    if chat {
        return Ok(true);
    }
    crate::feed::can_view_file(state, auth, file).await
}

/// An attachment as the office editor sees it, if the person may at least see it.
pub async fn office_file(state: &AppState, auth: &AuthUser, id: Uuid) -> AppResult<OfficeFile> {
    let meta = crate::routes::files::fetch(state, id).await?;
    if !crate::routes::files::can_read(state, &meta, auth).await? {
        return Err(AppError::NotFound("file"));
    }
    let (size, version, updated, created, owner): (i64, i32, Option<DateTime<Utc>>, DateTime<Utc>, Option<Uuid>) =
        sqlx::query_as("SELECT size_bytes, version, updated_at, created_at, owner_id FROM files WHERE id = $1").bind(id).fetch_one(&state.db).await?;
    Ok(OfficeFile { name: meta.original_name.clone(), file_id: id, size, version, updated_at: updated.unwrap_or(created), owner, can_edit: can_edit(state, auth, id).await? })
}

/// Saves edited content into the attachment, keeping what it held before.
pub async fn save(state: &AppState, auth: &AuthUser, id: Uuid, bytes: &[u8]) -> AppResult<(i32, DateTime<Utc>)> {
    if !can_edit(state, auth, id).await? {
        return Err(AppError::Forbidden);
    }
    let (name, mime): (String, String) = sqlx::query_as("SELECT original_name, mime_type FROM files WHERE id = $1").bind(id).fetch_one(&state.db).await?;
    // The new content is written as a file of its own, then takes the attachment's place.
    let fresh = crate::routes::files::store_bytes(state, Some(auth.id), &name, &mime, bytes).await?;
    let mut tx = state.db.begin().await?;
    sqlx::query(
        "INSERT INTO file_versions (file_id, version, storage_path, enc_key, size_bytes, sha256, saved_by)
         SELECT id, version, storage_path, enc_key, size_bytes, sha256, $2 FROM files WHERE id = $1",
    )
    .bind(id)
    .bind(auth.id)
    .execute(&mut *tx)
    .await?;
    let row: (i32, DateTime<Utc>) = sqlx::query_as(
        "UPDATE files f SET storage_path = n.storage_path, enc_key = n.enc_key, size_bytes = n.size_bytes, sha256 = n.sha256, version = f.version + 1, updated_at = now()
         FROM files n WHERE f.id = $1 AND n.id = $2 RETURNING f.version, f.updated_at",
    )
    .bind(id)
    .bind(fresh)
    .fetch_one(&mut *tx)
    .await?;
    // The row that carried the new content goes; its content now belongs to the attachment.
    sqlx::query("DELETE FROM files WHERE id = $1").bind(fresh).execute(&mut *tx).await?;
    // Its preview showed the old content.
    sqlx::query("DELETE FROM file_previews WHERE file_id = $1").bind(id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(row)
}
