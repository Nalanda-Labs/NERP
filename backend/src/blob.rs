//! Stored files on disk, encrypted at rest.
//!
//! With FILE_ENCRYPTION_KEY set, every file NERP stores is written encrypted: AES-256-GCM in
//! 64 KiB chunks (so a download, or a video being seeked, decrypts only the chunks it needs),
//! with a key of its own derived from the master key and a random salt. Layout:
//!
//!   "NERPENC1" | key id (8 bytes) | salt (16 bytes) | chunk 0 | chunk 1 | … | last chunk
//!
//! Each chunk is up to 64 KiB of the file plus a 16-byte tag; its nonce is the chunk number
//! with a flag on the last chunk, so chunks can't be reordered, dropped or the file cut short
//! without the read failing. `files.enc_key` records which key a file was written with (NULL:
//! not encrypted); a background job brings every file to the current key (encrypting files
//! from before, re-encrypting after a key change, or decrypting when the key is retired with
//! FILE_ENCRYPTION_OLD_KEYS and no current key).
//!
//! Removed files are moved to UPLOAD_DIR/.deleted/<day>/ and kept for FILE_KEEP_DELETED_DAYS,
//! so a database restored to an earlier moment (point-in-time restore) finds its files.

use aes_gcm::{
    aead::{AeadInOut, KeyInit, Nonce, Tag},
    Aes256Gcm,
};
use axum::body::Bytes;
use futures_util::Stream;
use hmac::{Hmac, Mac};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    io::{Error, ErrorKind, SeekFrom},
    path::{Path, PathBuf},
};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use uuid::Uuid;

use crate::state::AppState;

const MAGIC: &[u8; 8] = b"NERPENC1";
const HEADER: u64 = 32;
const CHUNK: usize = 64 * 1024;
const TAG: usize = 16;
pub const CONVERT_JOB: &str = "files.encrypt";
pub const PURGE_JOB: &str = "files.purge_deleted";

#[derive(Clone)]
pub struct FileKey {
    key: [u8; 32],
    id: [u8; 8],
}

impl FileKey {
    pub fn new(secret: &str) -> Self {
        let key: [u8; 32] = Sha256::digest(format!("nerp:file-key:v1:{secret}").as_bytes()).into();
        let mut id = [0u8; 8];
        id.copy_from_slice(&Sha256::digest([b"nerp:file-key-id:v1:".as_slice(), &key].concat())[..8]);
        Self { key, id }
    }
    pub fn id(&self) -> String {
        hex::encode(self.id)
    }
    fn cipher(&self, salt: &[u8]) -> Aes256Gcm {
        let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(&self.key).expect("any key length");
        mac.update(b"nerp:file:v1:");
        mac.update(salt);
        Aes256Gcm::new_from_slice(&mac.finalize().into_bytes()).expect("32-byte key")
    }
}

impl std::fmt::Debug for FileKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FileKey({})", self.id())
    }
}

/// The key new files are written with, and earlier keys files may still be under.
#[derive(Clone, Debug, Default)]
pub struct Keys {
    pub current: Option<FileKey>,
    pub old: Vec<FileKey>,
}

impl Keys {
    pub fn from_env() -> anyhow::Result<Self> {
        let current = match std::env::var("FILE_ENCRYPTION_KEY").ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
            Some(s) if s.len() < 32 => anyhow::bail!("FILE_ENCRYPTION_KEY must be at least 32 characters"),
            Some(s) => Some(FileKey::new(&s)),
            None => None,
        };
        let old = std::env::var("FILE_ENCRYPTION_OLD_KEYS")
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(FileKey::new)
            .collect();
        Ok(Self { current, old })
    }
    pub fn current_id(&self) -> Option<String> {
        self.current.as_ref().map(FileKey::id)
    }
    fn find(&self, id: &[u8]) -> Option<&FileKey> {
        self.current.iter().chain(self.old.iter()).find(|k| k.id == id)
    }
    pub fn knows(&self, id: &str) -> bool {
        hex::decode(id).ok().is_some_and(|b| self.find(&b).is_some())
    }
}

fn nonce(index: u64, last: bool) -> Nonce<Aes256Gcm> {
    let mut n = [0u8; 12];
    n[0] = last as u8;
    n[4..].copy_from_slice(&index.to_be_bytes());
    Nonce::<Aes256Gcm>::try_from(&n[..]).expect("12 bytes")
}

/// Writes a new stored file, encrypted when there is a key. Bytes go in as they come;
/// `finish` returns the size, SHA-256 (of the content) and the key id used.
pub struct Writer {
    out: tokio::fs::File,
    cipher: Option<Aes256Gcm>,
    key_id: Option<String>,
    buf: Vec<u8>,
    index: u64,
    hasher: Sha256,
    size: u64,
}

pub struct Written {
    pub size: u64,
    pub sha256: String,
    pub key: Option<String>,
}

impl Writer {
    pub async fn create(keys: &Keys, path: &Path) -> std::io::Result<Self> {
        Self::with_key(keys.current.as_ref(), path).await
    }

    async fn with_key(key: Option<&FileKey>, path: &Path) -> std::io::Result<Self> {
        let mut out = tokio::fs::File::create(path).await?;
        let cipher = match key {
            Some(k) => {
                let mut salt = [0u8; 16];
                rand::fill(&mut salt);
                out.write_all(&[MAGIC.as_slice(), &k.id, &salt].concat()).await?;
                Some(k.cipher(&salt))
            }
            None => None,
        };
        Ok(Self { out, cipher, key_id: key.map(FileKey::id), buf: Vec::with_capacity(CHUNK + 1), index: 0, hasher: Sha256::new(), size: 0 })
    }

    pub async fn write(&mut self, mut data: &[u8]) -> std::io::Result<()> {
        self.hasher.update(data);
        self.size += data.len() as u64;
        if self.cipher.is_none() {
            return self.out.write_all(data).await;
        }
        // A full chunk is written only once more follows, so the last one can be marked.
        while !data.is_empty() {
            let take = (CHUNK + 1 - self.buf.len()).min(data.len());
            self.buf.extend_from_slice(&data[..take]);
            data = &data[take..];
            if self.buf.len() > CHUNK {
                let rest = self.buf.split_off(CHUNK);
                self.seal(false).await?;
                self.buf = rest;
            }
        }
        Ok(())
    }

    async fn seal(&mut self, last: bool) -> std::io::Result<()> {
        let cipher = self.cipher.as_ref().expect("encrypting");
        let mut chunk = std::mem::take(&mut self.buf);
        let tag = cipher.encrypt_inout_detached(&nonce(self.index, last), b"", chunk.as_mut_slice().into()).map_err(|_| Error::other("encryption failed"))?;
        chunk.extend_from_slice(&tag);
        self.out.write_all(&chunk).await?;
        self.index += 1;
        self.buf = Vec::with_capacity(CHUNK + 1);
        Ok(())
    }

    pub async fn finish(mut self) -> std::io::Result<Written> {
        if self.cipher.is_some() {
            self.seal(true).await?;
        }
        self.out.flush().await?;
        self.out.sync_all().await?;
        Ok(Written { size: self.size, sha256: hex::encode(self.hasher.finalize()), key: self.key_id })
    }
}

/// An open stored file: its content size and how to read it.
pub struct Blob {
    file: tokio::fs::File,
    cipher: Option<Aes256Gcm>,
    pub size: u64,
    chunks: u64,
}

fn unknown_key() -> Error {
    Error::new(ErrorKind::InvalidData, "the file is encrypted with a key this server doesn't have (FILE_ENCRYPTION_KEY / FILE_ENCRYPTION_OLD_KEYS)")
}

pub async fn open(keys: &Keys, path: &Path, enc_key: Option<&str>) -> std::io::Result<Blob> {
    let mut file = tokio::fs::File::open(path).await?;
    let len = file.metadata().await?.len();
    if enc_key.is_none() {
        return Ok(Blob { file, cipher: None, size: len, chunks: 0 });
    }
    let mut head = [0u8; HEADER as usize];
    file.read_exact(&mut head).await.map_err(|_| Error::new(ErrorKind::InvalidData, "not an encrypted file"))?;
    if &head[..8] != MAGIC {
        return Err(Error::new(ErrorKind::InvalidData, "not an encrypted file"));
    }
    let key = keys.find(&head[8..16]).ok_or_else(unknown_key)?;
    let body = len - HEADER;
    let chunks = body.div_ceil((CHUNK + TAG) as u64).max(1);
    if body < chunks * TAG as u64 {
        return Err(Error::new(ErrorKind::InvalidData, "the encrypted file is cut short"));
    }
    Ok(Blob { file, cipher: Some(key.cipher(&head[16..])), size: body - chunks * TAG as u64, chunks })
}

impl Blob {
    async fn chunk(&mut self, index: u64) -> std::io::Result<Vec<u8>> {
        let cipher = self.cipher.as_ref().expect("encrypted");
        let at = HEADER + index * (CHUNK + TAG) as u64;
        let want = if index + 1 == self.chunks { (self.size - index * CHUNK as u64) as usize + TAG } else { CHUNK + TAG };
        self.file.seek(SeekFrom::Start(at)).await?;
        let mut buf = vec![0u8; want];
        self.file.read_exact(&mut buf).await?;
        let tag = buf.split_off(want - TAG);
        let tag = Tag::<Aes256Gcm>::try_from(&tag[..]).map_err(|_| Error::other("tag"))?;
        cipher
            .decrypt_inout_detached(&nonce(index, index + 1 == self.chunks), b"", buf.as_mut_slice().into(), &tag)
            .map_err(|_| Error::new(ErrorKind::InvalidData, "the encrypted file is damaged or was changed"))?;
        Ok(buf)
    }

    /// The bytes `start..start + len` of the content, as they are read.
    pub fn stream(self, start: u64, len: u64) -> impl Stream<Item = std::io::Result<Bytes>> + Send {
        futures_util::stream::try_unfold((self, start, len), |(mut b, pos, left)| async move {
            if left == 0 {
                return Ok(None);
            }
            if b.cipher.is_none() {
                b.file.seek(SeekFrom::Start(pos)).await?;
                let mut buf = vec![0u8; left.min(CHUNK as u64) as usize];
                b.file.read_exact(&mut buf).await?;
                let n = buf.len() as u64;
                return Ok(Some((Bytes::from(buf), (b, pos + n, left - n))));
            }
            let index = pos / CHUNK as u64;
            let plain = b.chunk(index).await?;
            let from = (pos - index * CHUNK as u64) as usize;
            let to = (from as u64 + left).min(plain.len() as u64) as usize;
            let part = Bytes::copy_from_slice(&plain[from..to]);
            let n = part.len() as u64;
            if n == 0 {
                return Err(Error::new(ErrorKind::UnexpectedEof, "past the end of the file"));
            }
            Ok(Some((part, (b, pos + n, left - n))))
        })
    }

    pub async fn read_all(mut self) -> std::io::Result<Vec<u8>> {
        use futures_util::TryStreamExt;
        let size = self.size;
        if size == 0 && self.cipher.is_some() {
            // Nothing to read, but the (empty) last chunk still proves the file is whole.
            self.chunk(0).await?;
        }
        let mut out = Vec::with_capacity(size as usize);
        let mut s = std::pin::pin!(self.stream(0, size));
        while let Some(part) = s.try_next().await? {
            out.extend_from_slice(&part);
        }
        Ok(out)
    }

    /// Copies the content to a plain file (for tools like ffmpeg that need a path).
    pub async fn copy_to(self, dest: &Path) -> std::io::Result<()> {
        use futures_util::TryStreamExt;
        let size = self.size;
        let mut out = tokio::fs::File::create(dest).await?;
        let mut s = std::pin::pin!(self.stream(0, size));
        while let Some(part) = s.try_next().await? {
            out.write_all(&part).await?;
        }
        out.flush().await
    }
}

/// A stored file's content (all of it).
pub async fn read(state: &AppState, rel: &str, enc_key: Option<&str>) -> std::io::Result<Vec<u8>> {
    open(&state.config.file_keys, &state.config.upload_dir.join(rel), enc_key).await?.read_all().await
}

/// Puts a removed file aside for FILE_KEEP_DELETED_DAYS (or removes it when that is 0).
pub async fn discard(state: &AppState, rel: &str) {
    let abs = state.config.upload_dir.join(rel);
    let days = state.config.keep_deleted_days;
    if days <= 0 {
        let _ = tokio::fs::remove_file(&abs).await;
        return;
    }
    let to = state.config.upload_dir.join(".deleted").join(chrono::Utc::now().format("%Y-%m-%d").to_string()).join(rel);
    if let Some(parent) = to.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    if tokio::fs::rename(&abs, &to).await.is_err() {
        let _ = tokio::fs::remove_file(&abs).await;
    }
}

/// Daily: removes put-aside files older than FILE_KEEP_DELETED_DAYS.
pub async fn purge_deleted(state: &AppState) -> anyhow::Result<()> {
    let dir = state.config.upload_dir.join(".deleted");
    let Ok(mut days) = tokio::fs::read_dir(&dir).await else { return Ok(()) };
    let cutoff = (chrono::Utc::now() - chrono::Duration::days(state.config.keep_deleted_days.max(0))).format("%Y-%m-%d").to_string();
    while let Some(d) = days.next_entry().await? {
        let name = d.file_name().to_string_lossy().to_string();
        if name.len() == 10 && name < cutoff {
            tokio::fs::remove_dir_all(d.path()).await?;
        }
    }
    Ok(())
}

/// How many files are under the current key, under another key, and not encrypted.
pub async fn status(state: &AppState) -> anyhow::Result<serde_json::Value> {
    let rows: Vec<(Option<String>, i64, i64)> =
        sqlx::query!(r#"SELECT enc_key AS "enc_key?", COUNT(*) AS "c1!", COALESCE(SUM(size_bytes), 0)::bigint AS "c2!" FROM files GROUP BY 1"#).map(|r| (r.enc_key, r.c1, r.c2)).fetch_all(&state.db).await?;
    let cur = state.config.file_keys.current_id();
    let (mut current, mut other, mut plain) = (0i64, 0i64, 0i64);
    for (k, n, _) in &rows {
        match k {
            None => plain += n,
            Some(k) if Some(k) == cur.as_ref() => current += n,
            Some(_) => other += n,
        }
    }
    let (deleted_files, deleted_bytes) = dir_usage(&state.config.upload_dir.join(".deleted"));
    Ok(json!({
        "enabled": cur.is_some(),
        "key_id": cur,
        "encrypted": current,
        "other_key": other,
        "not_encrypted": plain,
        "converting": (cur.is_some() && (plain > 0 || other > 0)) || (cur.is_none() && other > 0),
        "keep_deleted_days": state.config.keep_deleted_days,
        "deleted": { "files": deleted_files, "bytes": deleted_bytes },
    }))
}

fn dir_usage(dir: &Path) -> (u64, u64) {
    let (mut n, mut b) = (0, 0);
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            match e.metadata() {
                Ok(m) if m.is_dir() => stack.push(e.path()),
                Ok(m) => {
                    n += 1;
                    b += m.len();
                }
                Err(_) => {}
            }
        }
    }
    (n, b)
}

/// At start: refuse to run when files are under a key the server wasn't given (it would
/// serve errors for them), and start bringing files to the current key.
pub async fn check_at_start(state: &AppState) -> anyhow::Result<()> {
    let keys: Vec<String> = sqlx::query_scalar!(r#"SELECT DISTINCT enc_key AS "enc_key!" FROM files WHERE enc_key IS NOT NULL"#).fetch_all(&state.db).await?;
    let unknown: Vec<&String> = keys.iter().filter(|k| !state.config.file_keys.knows(k)).collect();
    if !unknown.is_empty() {
        anyhow::bail!(
            "stored files are encrypted with a key this server doesn't have (key id {}): set FILE_ENCRYPTION_KEY, \
             or list the earlier key in FILE_ENCRYPTION_OLD_KEYS",
            unknown.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
        );
    }
    if pending(state).await? > 0 {
        crate::jobs::enqueue(&state.db, CONVERT_JOB, json!({}), None).await?;
    }
    Ok(())
}

async fn pending(state: &AppState) -> anyhow::Result<i64> {
    Ok(sqlx::query_scalar!(r#"SELECT COUNT(*) AS "v!" FROM files WHERE enc_key IS DISTINCT FROM $1"#, state.config.file_keys.current_id())
        .fetch_one(&state.db)
        .await?)
}

/// Brings files to the current key, a batch at a time (and queues itself again while more
/// are left). A file is rewritten under a new name and the row switched over, so a download
/// already running keeps reading the old one.
pub async fn convert(state: &AppState) -> anyhow::Result<()> {
    // One run at a time (on any server sharing the database).
    let mut lock = state.db.acquire().await?;
    let got: bool = sqlx::query_scalar!(r#"SELECT pg_try_advisory_lock(hashtext('nerp:files.encrypt')) AS "v!""#).fetch_one(&mut *lock).await?;
    if !got {
        return Ok(());
    }
    let result = convert_batch(state).await;
    let _ = sqlx::query("SELECT pg_advisory_unlock(hashtext('nerp:files.encrypt'))").execute(&mut *lock).await;
    result
}

async fn convert_batch(state: &AppState) -> anyhow::Result<()> {
    let cur = state.config.file_keys.current_id();
    let batch: Vec<(Uuid, String, Option<String>)> =
        sqlx::query!(r#"SELECT id, storage_path, enc_key FROM files WHERE enc_key IS DISTINCT FROM $1 ORDER BY created_at LIMIT 100"#, cur.as_deref()).map(|r| (r.id, r.storage_path, r.enc_key))
            .fetch_all(&state.db)
            .await?;
    let mut failed = 0;
    let started = std::time::Instant::now();
    let mut stopped_early = false;
    for (id, rel, enc) in &batch {
        // Jobs have a time limit: stop in good time and go on in the next run.
        if started.elapsed() > std::time::Duration::from_secs(150) {
            stopped_early = true;
            break;
        }
        if let Err(e) = convert_one(state, *id, rel, enc.as_deref()).await {
            failed += 1;
            tracing::warn!(file = %id, error = %e, "could not bring a stored file to the current encryption key");
        }
    }
    // More to do (and not only files that keep failing): go on.
    if stopped_early || (batch.len() == 100 && failed < batch.len()) {
        crate::jobs::enqueue(&state.db, CONVERT_JOB, json!({}), None).await?;
    }
    Ok(())
}

async fn convert_one(state: &AppState, id: Uuid, rel: &str, enc: Option<&str>) -> anyhow::Result<()> {
    let keys = &state.config.file_keys;
    let dir = Path::new(rel).parent().map(Path::to_path_buf).unwrap_or_default();
    let tag = keys.current_id().unwrap_or_else(|| "plain".into());
    let new_rel: PathBuf = dir.join(format!("{id}.{tag}"));
    let new_abs = state.config.upload_dir.join(&new_rel);
    let tmp = new_abs.with_extension(format!("part-{}", Uuid::new_v4().simple()));
    let result: anyhow::Result<Written> = async {
        let blob = match open(keys, &state.config.upload_dir.join(rel), enc).await {
            Ok(b) => b,
            // Gone from disk: nothing to convert; mark it so it isn't tried again.
            Err(e) if e.kind() == ErrorKind::NotFound => anyhow::bail!("missing on disk"),
            Err(e) => return Err(e.into()),
        };
        let size = blob.size;
        let mut w = Writer::create(keys, &tmp).await?;
        let mut s = std::pin::pin!(blob.stream(0, size));
        use futures_util::TryStreamExt;
        while let Some(part) = s.try_next().await? {
            w.write(&part).await?;
        }
        Ok(w.finish().await?)
    }
    .await;
    let written = match result {
        Ok(w) => w,
        Err(e) => {
            let _ = tokio::fs::remove_file(&tmp).await;
            if e.to_string() == "missing on disk" {
                sqlx::query!("UPDATE files SET enc_key = $2 WHERE id = $1", id, keys.current_id()).execute(&state.db).await?;
            }
            return Err(e);
        }
    };
    tokio::fs::rename(&tmp, &new_abs).await?;
    let switched = sqlx::query("UPDATE files SET storage_path = $2, enc_key = $3 WHERE id = $1 AND storage_path = $4")
        .bind(id)
        .bind(new_rel.to_string_lossy().as_ref())
        .bind(&written.key)
        .bind(rel)
        .execute(&state.db)
        .await?
        .rows_affected();
    if switched == 1 {
        discard(state, rel).await;
    } else {
        let _ = tokio::fs::remove_file(&new_abs).await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn round_trip(keys: &Keys, len: usize) {
        let dir = std::env::temp_dir().join(format!("nerp-blob-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let path = dir.join("f");
        let data: Vec<u8> = (0..len).map(|i| (i * 7 % 251) as u8).collect();
        let mut w = Writer::create(keys, &path).await.unwrap();
        for part in data.chunks(10_000) {
            w.write(part).await.unwrap();
        }
        let done = w.finish().await.unwrap();
        assert_eq!(done.size, len as u64);
        let enc = done.key.clone();
        let raw = tokio::fs::read(&path).await.unwrap();
        if enc.is_some() && len > 64 {
            assert!(!raw.windows(64).any(|w| w == &data[..64]), "content is not on disk in the clear");
        }
        let b = open(keys, &path, enc.as_deref()).await.unwrap();
        assert_eq!(b.size, len as u64);
        assert_eq!(b.read_all().await.unwrap(), data);
        // Ranges across chunk borders.
        for (start, n) in [(0u64, 1u64), (65_535, 2), (70_000, 100_000), (len as u64 / 2, len as u64 / 2)] {
            if start + n > len as u64 || n == 0 {
                continue;
            }
            use futures_util::TryStreamExt;
            let b = open(keys, &path, enc.as_deref()).await.unwrap();
            let got: Vec<u8> = b.stream(start, n).try_collect::<Vec<_>>().await.unwrap().concat();
            assert_eq!(got, &data[start as usize..(start + n) as usize]);
        }
        if enc.is_some() {
            // Changed, cut short, or read with another key: refused.
            let mut bad = raw.clone();
            let at = bad.len() / 2;
            bad[at] ^= 1;
            tokio::fs::write(&path, &bad).await.unwrap();
            assert!(open(keys, &path, enc.as_deref()).await.unwrap().read_all().await.is_err());
            if len >= 2 * CHUNK {
                tokio::fs::write(&path, &raw[..raw.len() - (CHUNK + TAG)]).await.unwrap();
                assert!(open(keys, &path, enc.as_deref()).await.unwrap().read_all().await.is_err(), "a dropped last chunk is noticed");
            }
            tokio::fs::write(&path, &raw).await.unwrap();
            let other = Keys { current: Some(FileKey::new("another key, at least 32 chars long")), old: vec![] };
            assert!(open(&other, &path, enc.as_deref()).await.is_err());
        }
        tokio::fs::remove_dir_all(&dir).await.unwrap();
    }

    #[tokio::test]
    async fn encrypted_and_plain() {
        let keys = Keys { current: Some(FileKey::new("a test key that is long enough 0123")), old: vec![] };
        for len in [0, 1, CHUNK - 1, CHUNK, CHUNK + 1, 2 * CHUNK, 300_001] {
            round_trip(&keys, len).await;
            round_trip(&Keys::default(), len).await;
        }
    }
}
