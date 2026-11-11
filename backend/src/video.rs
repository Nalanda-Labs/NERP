//! Videos for every browser: an uploaded video (from a phone, a camera, a screen recording, a
//! video message) gets a web copy — H.264 video and AAC sound in an MP4 that starts playing
//! before it has all arrived, at most 1080p — and a poster frame, made in the background with
//! ffmpeg. Videos that already play everywhere keep their original. Videos larger than
//! `VIDEO_CONVERT_MAX_MB` (default 2048) or longer than four hours aren't converted.
//!
//! Players ask for `/files/{id}/video` (the web copy once there, the original until then) and
//! `/files/{id}/poster`; whoever may read the file may read these.

use axum::{
    extract::{Path, State},
    http::HeaderMap,
    response::Response,
    routing::get,
    Json, Router,
};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::process::Command;
use uuid::Uuid;

use crate::{
    auth::MaybeAuthUser,
    error::{AppError, AppResult},
    routes::files::{can_read, fetch, FileMeta},
    state::AppState,
};

pub const JOB: &str = "files.video";
const MAX_SECS: f64 = 4.0 * 3600.0;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/files/{id}/video", get(video))
        .route("/files/{id}/poster", get(poster))
        .route("/files/{id}/video-status", get(status))
}

const VIDEO_EXT: &[&str] = &["mp4", "m4v", "mov", "webm", "mkv", "avi", "wmv", "flv", "mpg", "mpeg", "3gp", "ogv", "ts", "mts"];

pub fn is_video(mime: &str, name: &str) -> bool {
    mime.starts_with("video/") || name.rsplit_once('.').is_some_and(|(_, e)| VIDEO_EXT.contains(&e.to_ascii_lowercase().as_str()))
}

fn max_bytes() -> i64 {
    std::env::var("VIDEO_CONVERT_MAX_MB").ok().and_then(|v| v.parse::<i64>().ok()).unwrap_or(2048) * 1024 * 1024
}

/// Queues a video's web copy.
pub async fn queue(state: &AppState, file: Uuid) {
    let r = async {
        sqlx::query("INSERT INTO file_renditions (file_id, status) VALUES ($1, 'pending') ON CONFLICT (file_id) DO NOTHING").bind(file).execute(&state.db).await?;
        crate::jobs::enqueue(&state.db, JOB, json!({ "file_id": file }), None).await?;
        state.job_wakeup.notify_one();
        Ok::<_, AppError>(())
    }
    .await;
    if let Err(e) = r {
        tracing::warn!(file = %file, error = ?e, "could not queue a video's web copy");
    }
}

async fn set(state: &AppState, file: Uuid, status: &str, error: Option<String>) -> anyhow::Result<()> {
    sqlx::query("UPDATE file_renditions SET status = $2, error = $3, updated_at = now() WHERE file_id = $1").bind(file).bind(status).bind(error).execute(&state.db).await?;
    Ok(())
}

/// What ffprobe says: (duration, width, height, video codec, audio codec, container).
async fn probe(path: &std::path::Path) -> anyhow::Result<(f64, i64, i64, String, Option<String>, String)> {
    let out = tokio::time::timeout(Duration::from_secs(60), Command::new("ffprobe").args(["-v", "error", "-print_format", "json", "-show_streams", "-show_format"]).arg(path).kill_on_drop(true).output()).await??;
    if !out.status.success() {
        anyhow::bail!("not a video ffmpeg can read");
    }
    let v: Value = serde_json::from_slice(&out.stdout)?;
    let streams = v["streams"].as_array().cloned().unwrap_or_default();
    let video = streams.iter().find(|s| s["codec_type"] == "video" && s["disposition"]["attached_pic"] != 1).ok_or_else(|| anyhow::anyhow!("no video in the file"))?;
    let audio = streams.iter().find(|s| s["codec_type"] == "audio");
    let duration = v["format"]["duration"].as_str().and_then(|d| d.parse().ok()).unwrap_or(0.0);
    Ok((
        duration,
        video["width"].as_i64().unwrap_or(0),
        video["height"].as_i64().unwrap_or(0),
        video["codec_name"].as_str().unwrap_or("").to_string(),
        audio.and_then(|a| a["codec_name"].as_str()).map(String::from),
        v["format"]["format_name"].as_str().unwrap_or("").to_string(),
    ))
}

async fn ffmpeg(args: &[&str], secs: u64) -> anyhow::Result<()> {
    let out = tokio::time::timeout(Duration::from_secs(secs), Command::new("ffmpeg").args(["-hide_banner", "-loglevel", "error", "-y"]).args(args).kill_on_drop(true).output()).await??;
    if !out.status.success() {
        anyhow::bail!(String::from_utf8_lossy(&out.stderr).lines().last().unwrap_or("ffmpeg failed").to_string());
    }
    Ok(())
}

/// Conversions run one at a time, outside the job queue (they can take longer than a job's
/// lease); the job only hands the video over.
static CONVERTING: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

/// Background job: starts a video's conversion (if nobody else has).
pub async fn run_job(state: &AppState, payload: &Value) -> anyhow::Result<()> {
    let id: Uuid = serde_json::from_value(payload["file_id"].clone())?;
    let claimed = sqlx::query("UPDATE file_renditions SET status = 'converting', updated_at = now() WHERE file_id = $1 AND status = 'pending'").bind(id).execute(&state.db).await?.rows_affected();
    if claimed == 1 {
        let state = state.clone();
        tokio::spawn(async move {
            let _permit = CONVERTING.acquire().await;
            if let Err(e) = convert(&state, id).await {
                tracing::warn!(file = %id, error = ?e, "video conversion failed");
                let _ = set(&state, id, "failed", Some(e.to_string().chars().take(300).collect())).await;
            }
        });
    }
    Ok(())
}

/// At start-up: conversions cut off by a restart start again.
pub async fn resume(state: &AppState) {
    let ids: Vec<Uuid> = sqlx::query_scalar("UPDATE file_renditions SET status = 'pending' WHERE status = 'converting' RETURNING file_id").fetch_all(&state.db).await.unwrap_or_default();
    for id in ids {
        let _ = crate::jobs::enqueue(&state.db, JOB, json!({ "file_id": id }), None).await;
    }
}

/// Makes a video's web copy and poster.
async fn convert(state: &AppState, id: Uuid) -> anyhow::Result<()> {
    let Ok(f) = fetch(state, id).await else { return Ok(()) };
    if f.size_bytes > max_bytes() {
        return set(state, id, "too_big", Some(format!("larger than {} MB", max_bytes() / 1024 / 1024))).await;
    }
    if Command::new("ffprobe").arg("-version").output().await.is_err() {
        return set(state, id, "unavailable", Some("ffmpeg isn't installed".into())).await;
    }
    // ffmpeg reads a path: an encrypted file is decrypted to a private temporary copy.
    let tmp = std::env::temp_dir().join(format!("nerp-video-{id}"));
    let src = if f.enc_key.is_some() {
        use std::os::unix::fs::DirBuilderExt;
        let _ = tokio::fs::remove_dir_all(&tmp).await;
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&tmp)?;
        let p = tmp.join("source");
        if let Err(e) = crate::routes::files::copy_out(state, &f, &p).await {
            let _ = tokio::fs::remove_dir_all(&tmp).await;
            return set(state, id, "failed", Some(e.to_string())).await;
        }
        p
    } else {
        state.config.upload_dir.join(&f.storage_path)
    };
    let (duration, width, height, vcodec, acodec, container) = match probe(&src).await {
        Ok(p) => p,
        Err(e) => {
            let _ = tokio::fs::remove_dir_all(&tmp).await;
            return set(state, id, "failed", Some(e.to_string())).await;
        }
    };
    if duration > MAX_SECS {
        let _ = tokio::fs::remove_dir_all(&tmp).await;
        return set(state, id, "too_big", Some("longer than four hours".into())).await;
    }
    sqlx::query("UPDATE file_renditions SET duration_secs = $2, width = $3, height = $4 WHERE file_id = $1").bind(id).bind(duration as f32).bind(width as i32).bind(height as i32).execute(&state.db).await?;
    tokio::fs::create_dir_all(&tmp).await?;
    let result: anyhow::Result<()> = async {
        // Poster: a frame a second in (or from the middle of a short clip).
        let at = format!("{:.2}", (duration / 2.0).min(1.0));
        let poster = tmp.join("poster.jpg");
        let src_s = src.to_string_lossy().to_string();
        if ffmpeg(&["-ss", &at, "-i", &src_s, "-frames:v", "1", "-vf", "scale='min(1280,iw)':-2", "-q:v", "4", &poster.to_string_lossy()], 120).await.is_ok() {
            let p = crate::routes::files::store_path(state, f.owner_id, &format!("{}.jpg", stem(&f.original_name)), "image/jpeg", &poster).await?;
            sqlx::query("UPDATE file_renditions SET poster_file_id = $2 WHERE file_id = $1").bind(id).bind(p).execute(&state.db).await?;
        }
        // Already playable everywhere: H.264 (+AAC or no sound) in MP4, at most 1080p.
        let playable = vcodec == "h264" && acodec.as_deref().is_none_or(|a| a == "aac") && container.contains("mp4") && height <= 1080 && width <= 1920;
        if playable {
            return set(state, id, "original", None).await;
        }
        let out = tmp.join("web.mp4");
        let timeout = (duration * 4.0).max(600.0) as u64;
        let mut args: Vec<String> = vec![
            "-i".into(), src_s.clone(),
            "-map".into(), "0:v:0".into(), "-map".into(), "0:a:0?".into(),
            "-vf".into(), "scale='min(1920,iw)':'min(1080,ih)':force_original_aspect_ratio=decrease,scale=trunc(iw/2)*2:trunc(ih/2)*2".into(),
            "-c:v".into(), "libx264".into(), "-preset".into(), "veryfast".into(), "-crf".into(), "23".into(), "-pix_fmt".into(), "yuv420p".into(),
            "-c:a".into(), "aac".into(), "-b:a".into(), "128k".into(),
            "-movflags".into(), "+faststart".into(),
        ];
        args.push(out.to_string_lossy().to_string());
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        ffmpeg(&refs, timeout).await?;
        let web = crate::routes::files::store_path(state, f.owner_id, &format!("{}.mp4", stem(&f.original_name)), "video/mp4", &out).await?;
        sqlx::query("UPDATE file_renditions SET web_file_id = $2, status = 'ready', error = NULL, updated_at = now() WHERE file_id = $1").bind(id).bind(web).execute(&state.db).await?;
        Ok(())
    }
    .await;
    let _ = tokio::fs::remove_dir_all(&tmp).await;
    if let Err(e) = result {
        set(state, id, "failed", Some(e.to_string().chars().take(300).collect())).await?;
    }
    Ok(())
}

fn stem(name: &str) -> &str {
    name.rsplit_once('.').map(|(s, _)| s).unwrap_or(name)
}

/// The file, if the caller may read it (as `/files/{id}/content` decides).
async fn readable(state: &AppState, auth: &Option<crate::auth::AuthUser>, id: Uuid) -> AppResult<FileMeta> {
    let f = fetch(state, id).await?;
    let ok = f.is_public
        || match auth {
            Some(a) => can_read(state, &f, a).await?,
            None => false,
        };
    if !ok {
        return Err(if auth.is_none() { AppError::Unauthorized } else { AppError::NotFound("file") });
    }
    Ok(f)
}

async fn rendition(state: &AppState, id: Uuid) -> AppResult<Option<(String, Option<Uuid>, Option<Uuid>, Option<f32>, Option<i32>, Option<i32>)>> {
    Ok(sqlx::query_as("SELECT status, web_file_id, poster_file_id, duration_secs, width, height FROM file_renditions WHERE file_id = $1").bind(id).fetch_optional(&state.db).await?)
}

/// The video to play: the web copy once made, else the original.
async fn video(State(state): State<AppState>, MaybeAuthUser(auth): MaybeAuthUser, Path(id): Path<Uuid>, headers: HeaderMap) -> AppResult<Response> {
    let f = readable(&state, &auth, id).await?;
    let web = rendition(&state, id).await?.and_then(|r| r.1);
    match web {
        Some(w) => crate::routes::files::stream_id(&state, w, &format!("{}.mp4", stem(&f.original_name)), &headers).await,
        None => crate::routes::files::stream_id(&state, id, &f.original_name, &headers).await,
    }
}

async fn poster(State(state): State<AppState>, MaybeAuthUser(auth): MaybeAuthUser, Path(id): Path<Uuid>, headers: HeaderMap) -> AppResult<Response> {
    let f = readable(&state, &auth, id).await?;
    let p = rendition(&state, id).await?.and_then(|r| r.2).ok_or(AppError::NotFound("poster"))?;
    crate::routes::files::stream_id(&state, p, &format!("{}.jpg", stem(&f.original_name)), &headers).await
}

/// How the web copy stands: pending, ready, original, too_big, failed or unavailable.
async fn status(State(state): State<AppState>, MaybeAuthUser(auth): MaybeAuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    readable(&state, &auth, id).await?;
    Ok(Json(match rendition(&state, id).await? {
        Some((st, web, poster, dur, w, h)) => json!({ "status": st, "web": web.is_some(), "poster": poster.is_some(), "duration_secs": dur, "width": w, "height": h }),
        None => json!({ "status": "none" }),
    }))
}

#[cfg(test)]
mod tests {
    #[test]
    fn videos() {
        assert!(super::is_video("video/quicktime", "a.mov"));
        assert!(super::is_video("application/octet-stream", "Clip.MKV"));
        assert!(!super::is_video("image/png", "a.png"));
    }
}
