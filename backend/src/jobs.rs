//! Background jobs, stored in the `jobs` table so they survive restarts and can run on
//! any instance.
//!
//! Claiming is optimistic (`UPDATE … WHERE status = 'pending'` and check that one row
//! changed), so two workers never take the same job. A running job holds a lease (`locked_until`); if its worker dies, the
//! reaper puts it back in the queue.
//!
//! Recurring maintenance is scheduled through a Redis lock, so each task runs once per
//! interval no matter how many instances there are.

use anyhow::{anyhow, bail, Result};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};
use tokio::sync::Semaphore;
use uuid::Uuid;

use crate::{mail::Email, state::AppState};

pub const EMAIL_SEND: &str = "email.send";
pub const CLEANUP_NOTIFICATIONS: &str = "cleanup.notifications";
pub const CLEANUP_JOBS: &str = "cleanup.jobs";

const LEASE_SECS: i64 = 300;
const HANDLER_TIMEOUT: Duration = Duration::from_secs(240);

/// Recurring tasks: (job kind, interval).
const RECURRING: &[(&str, u64)] = &[
    (CLEANUP_NOTIFICATIONS, 24 * 3600),
    (CLEANUP_JOBS, 24 * 3600),
    (crate::tasks::reminders::JOB, 300),
    (crate::tasks::templates::JOB, 60),
    (crate::crm::REMIND_JOB, 60),
    (crate::crm::PURGE_JOB, 24 * 3600),
    (crate::crm::email::POLL_JOB, 60),
    (crate::crm::automation::SCHEDULE_JOB, 60),
    (crate::marketing::DUE_JOB, 60),
    (crate::crm::recurring::JOB, 60),
    (crate::calendar::REMIND_JOB, 60),
    (crate::calendar::dav::CLEANUP_JOB, 24 * 3600),
    (crate::calendar::sync::SYNC_JOB, 300),
    (crate::chat::MAINTENANCE_JOB, 60),
    (crate::hr::reports::REMIND_JOB, 3600),
    (crate::bi::REFRESH_JOB, 3600),
    (crate::calls::CLEANUP_JOB, 60),
    (crate::calls::RECORDINGS_JOB, 20),
    (crate::drive::PURGE_JOB, 24 * 3600),
    (crate::blob::PURGE_JOB, 24 * 3600),
    (crate::blob::CONVERT_JOB, 3600),
    (crate::tracking::CLEANUP_JOB, 24 * 3600),
    (crate::directory::SYNC_JOB, 3600),
    (crate::im::MAINTENANCE_JOB, 300),
    (crate::tasks::depth::TRASH_PURGE_JOB, 24 * 3600),
    (crate::booking::REMIND_JOB, 300),
    (crate::training::REMIND_JOB, 3600),
    (crate::support::SLA_JOB, 60),
];

/// Adds a job to the queue. Pass a transaction to enqueue atomically with other writes.
pub async fn enqueue<'e, E: sqlx::PgExecutor<'e>>(
    executor: E,
    kind: &str,
    payload: Value,
    run_at: Option<DateTime<Utc>>,
) -> sqlx::Result<Uuid> {
    sqlx::query_scalar(
        "INSERT INTO jobs (kind, payload, run_at) VALUES ($1, $2, COALESCE($3, now())) RETURNING id",
    )
    .bind(kind)
    .bind(payload)
    .bind(run_at)
    .fetch_one(executor)
    .await
}

/// Convenience wrapper: queue an email and wake the local worker.
pub async fn send_email(state: &AppState, email: Email) -> sqlx::Result<Uuid> {
    let id = enqueue(&state.db, EMAIL_SEND, serde_json::to_value(email).unwrap(), None).await?;
    state.job_wakeup.notify_one();
    Ok(id)
}

#[derive(sqlx::FromRow)]
struct Claimed {
    id: Uuid,
    kind: String,
    payload: Value,
    attempts: i32,
    max_attempts: i32,
}

async fn claim(state: &AppState, limit: i64) -> Result<Vec<Claimed>> {
    let candidates: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM jobs WHERE status = 'pending' AND run_at <= now() ORDER BY run_at LIMIT $1",
    )
    .bind(limit)
    .fetch_all(&state.db)
    .await?;

    let mut claimed = Vec::new();
    for id in candidates {
        let job: Option<Claimed> = sqlx::query_as(
            "UPDATE jobs SET status = 'running', attempts = attempts + 1, locked_by = $2,
                 locked_until = now() + make_interval(secs => $3), updated_at = now()
             WHERE id = $1 AND status = 'pending'
             RETURNING id, kind, payload, attempts, max_attempts",
        )
        .bind(id)
        .bind(&state.instance_id)
        .bind(LEASE_SECS as f64)
        .fetch_optional(&state.db)
        .await?;
        claimed.extend(job);
    }
    Ok(claimed)
}

async fn finish(state: &AppState, job: &Claimed, result: Result<()>) {
    let outcome = match result {
        Ok(()) => {
            sqlx::query(
                "UPDATE jobs SET status = 'done', locked_by = NULL, locked_until = NULL, last_error = NULL,
                     updated_at = now() WHERE id = $1",
            )
            .bind(job.id)
            .execute(&state.db)
            .await
        }
        Err(e) => {
            let error = format!("{e:#}");
            let give_up = job.attempts >= job.max_attempts;
            // 20s, 40s, 80s … capped at one hour.
            let backoff = (10i64 << job.attempts.clamp(1, 12)).min(3600);
            if give_up {
                tracing::error!(job = %job.id, kind = job.kind, error, "job failed permanently");
            } else {
                tracing::warn!(job = %job.id, kind = job.kind, attempt = job.attempts, error, "job failed; will retry");
            }
            sqlx::query(
                "UPDATE jobs SET status = $2, last_error = $3, locked_by = NULL, locked_until = NULL,
                     run_at = now() + make_interval(secs => $4), updated_at = now()
                 WHERE id = $1",
            )
            .bind(job.id)
            .bind(if give_up { "failed" } else { "pending" })
            .bind(error)
            .bind(backoff as f64)
            .execute(&state.db)
            .await
        }
    };
    if let Err(e) = outcome {
        tracing::error!(job = %job.id, error = ?e, "could not record job outcome");
    }
}

async fn run(state: &AppState, kind: &str, payload: &Value) -> Result<()> {
    match kind {
        EMAIL_SEND => {
            let email: Email = serde_json::from_value(payload.clone())?;
            let mut files = Vec::new();
            for id in &email.attachments {
                let (name, mime, _, bytes) = crate::routes::files::read_bytes(state, *id).await.map_err(|e| anyhow!("attachment {id}: {e}"))?;
                files.push(crate::mail::Attachment { name, mime, bytes });
            }
            state.mailer.send(&email, files).await
        }
        CLEANUP_NOTIFICATIONS => {
            let deleted = sqlx::query(
                "DELETE FROM notifications
                 WHERE (read_at IS NOT NULL AND created_at < now() - interval '90 days')
                    OR created_at < now() - interval '365 days'",
            )
            .execute(&state.db)
            .await?
            .rows_affected();
            tracing::info!(deleted, "cleaned up old notifications");
            Ok(())
        }
        CLEANUP_JOBS => {
            let deleted = sqlx::query(
                "DELETE FROM jobs
                 WHERE (status = 'done' AND updated_at < now() - interval '7 days')
                    OR (status = 'failed' AND updated_at < now() - interval '30 days')",
            )
            .execute(&state.db)
            .await?
            .rows_affected();
            sqlx::query("DELETE FROM webhook_deliveries WHERE status <> 'pending' AND created_at < now() - interval '30 days'").execute(&state.db).await?;
            tracing::info!(deleted, "cleaned up old jobs");
            Ok(())
        }
        crate::tasks::reminders::JOB => crate::tasks::reminders::run(state).await,
        crate::tasks::templates::JOB => crate::tasks::templates::run_due(state).await,
        crate::crm::REMIND_JOB => crate::crm::run_reminders(state).await,
        crate::crm::PURGE_JOB => crate::crm::run_purge(state).await,
        crate::crm::automation::JOB => crate::crm::automation::run_job(state, payload).await,
        crate::crm::sign_templates::JOB => crate::crm::sign_templates::run_job(state, payload).await,
        crate::api::webhooks::JOB => crate::api::webhooks::run_job(state, payload).await,
        crate::marketing::JOB => crate::marketing::run_job(state, payload).await,
        crate::marketing::DUE_JOB => crate::marketing::run_due(state).await,
        crate::marketing::GENERATOR_JOB => crate::marketing::run_generator(state, payload).await,
        crate::tracking::CLEANUP_JOB => crate::tracking::cleanup(state).await,
        crate::video::JOB => crate::video::run_job(state, payload).await,
        crate::crm::recurring::JOB => crate::crm::recurring::run_due(state).await,
        crate::calendar::REMIND_JOB => crate::calendar::run_reminders(state).await,
        crate::calendar::dav::CLEANUP_JOB => crate::calendar::dav::cleanup(state).await,
        crate::calendar::sync::SYNC_JOB => crate::calendar::sync::run_due(state).await,
        crate::chat::MAINTENANCE_JOB => crate::chat::maintenance(state).await,
        crate::hr::reports::REMIND_JOB => crate::hr::reports::remind(state).await,
        crate::bi::REFRESH_JOB => crate::bi::refresh_due(state).await,
        crate::calls::CLEANUP_JOB => crate::calls::run_cleanup(state).await,
        crate::calls::RECORDINGS_JOB => crate::calls::run_recordings(state).await,
        crate::workflows::RESUME_JOB => crate::workflows::resume(state, payload).await,
        crate::drive::PURGE_JOB => crate::drive::run_purge(state).await,
        crate::blob::PURGE_JOB => crate::blob::purge_deleted(state).await,
        crate::blob::CONVERT_JOB => crate::blob::convert(state).await,
        crate::directory::SYNC_JOB => crate::directory::ldap::run_sync(state).await,
        crate::im::MAINTENANCE_JOB => crate::im::run_maintenance(state).await,
        crate::tasks::automation::JOB => crate::tasks::automation::run_job(state, payload).await,
        crate::tasks::depth::TRASH_PURGE_JOB => crate::tasks::depth::run_trash_purge(state).await,
        crate::booking::REMIND_JOB => crate::booking::run_reminders(state).await,
        crate::training::REMIND_JOB => crate::training::run_reminders(state).await,
        crate::support::SLA_JOB => crate::support::sla::run_job(state).await,
        crate::support::MAIL_SEND_JOB => crate::support::mail::run_send(state, payload).await,
        crate::crm::automation::SCHEDULE_JOB => crate::crm::automation::schedules_due(state).await,
        crate::crm::automation::sla::JOB => crate::crm::automation::sla::run_job(state, payload).await,
        crate::crm::email::POLL_JOB => crate::crm::email::run_poll(state).await,
        crate::crm::email::SYNC_JOB => crate::crm::email::run_sync(state, payload).await,
        crate::crm::email::SEND_JOB => crate::crm::email::run_send(state, payload).await,
        crate::crm::docs::JOB => crate::crm::docs::run_job(state, payload).await,
        crate::crm::IMPORT_JOB => crate::crm::run_import(state, payload).await,
        crate::crm::migrate::JOB => crate::crm::migrate::run_job(state, payload).await,
        other => bail!("no handler for job kind {other:?}"),
    }
}

/// Returns jobs whose worker vanished to the queue.
async fn reap(state: &AppState) -> Result<()> {
    let n = sqlx::query(
        "UPDATE jobs SET status = 'pending', locked_by = NULL, locked_until = NULL, updated_at = now()
         WHERE status = 'running' AND locked_until < now()",
    )
    .execute(&state.db)
    .await?
    .rows_affected();
    if n > 0 {
        tracing::warn!(count = n, "re-queued jobs whose worker stopped responding");
    }
    Ok(())
}

async fn schedule_recurring(state: &AppState) -> Result<()> {
    let mut redis = state.redis.clone();
    for (kind, every) in RECURRING {
        let acquired: Option<String> = redis::cmd("SET")
            .arg(format!("jobs:recurring:{kind}"))
            .arg(&state.instance_id)
            .arg("NX")
            .arg("EX")
            // A little under the interval: a lock still alive at the next check would skip a run.
            .arg(every.saturating_sub(5).max(1))
            .query_async(&mut redis)
            .await?;
        if acquired.is_some() {
            enqueue(&state.db, kind, json!({}), None).await?;
        }
    }
    Ok(())
}

/// Starts the worker loop. Runs until the process exits.
pub fn start_worker(state: AppState) {
    let concurrency = state.config.worker_concurrency.max(1);
    let permits = Arc::new(Semaphore::new(concurrency));
    tokio::spawn(async move {
        tracing::info!(instance = state.instance_id, concurrency, "background job worker started");
        // Often enough that minute-jobs start within ~15 seconds of being due.
        let mut maintenance = tokio::time::interval(Duration::from_secs(15));
        loop {
            tokio::select! {
                _ = maintenance.tick() => {
                    if let Err(e) = reap(&state).await { tracing::warn!(error = ?e, "job reaper failed"); }
                    if let Err(e) = schedule_recurring(&state).await { tracing::warn!(error = ?e, "recurring job scheduling failed"); }
                }
                _ = state.job_wakeup.notified() => {}
                _ = tokio::time::sleep(Duration::from_secs(2)) => {}
            }

            let free = permits.available_permits() as i64;
            if free == 0 {
                continue;
            }
            let jobs = match claim(&state, free).await {
                Ok(j) => j,
                Err(e) => {
                    tracing::warn!(error = ?e, "could not claim jobs");
                    continue;
                }
            };
            for job in jobs {
                let permit = permits.clone().acquire_owned().await.expect("semaphore closed");
                let state = state.clone();
                tokio::spawn(async move {
                    let result = tokio::time::timeout(HANDLER_TIMEOUT, run(&state, &job.kind, &job.payload))
                        .await
                        .unwrap_or_else(|_| Err(anyhow!("timed out after {}s", HANDLER_TIMEOUT.as_secs())));
                    finish(&state, &job, result).await;
                    drop(permit);
                    // There may be more work waiting.
                    state.job_wakeup.notify_one();
                });
            }
        }
    });
}

#[derive(Serialize, sqlx::FromRow)]
pub struct JobRow {
    pub id: Uuid,
    pub kind: String,
    pub payload: Value,
    pub status: String,
    pub run_at: DateTime<Utc>,
    pub attempts: i32,
    pub max_attempts: i32,
    pub locked_by: Option<String>,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
