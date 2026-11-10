use serde_json::Value;
use uuid::Uuid;

/// Writes an audit record. Failures are logged rather than propagated so an
/// audit hiccup never undoes the action the user just completed.
pub async fn record<'e, E>(
    executor: E,
    actor_id: Option<Uuid>,
    action: &str,
    entity_type: &str,
    entity_id: Option<String>,
    details: Value,
    ip: Option<&str>,
) where
    E: sqlx::PgExecutor<'e>,
{
    let result = sqlx::query(
        "INSERT INTO audit_log (actor_id, action, entity_type, entity_id, details, ip)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(actor_id)
    .bind(action)
    .bind(entity_type)
    .bind(entity_id)
    .bind(details)
    .bind(ip)
    .execute(executor)
    .await;
    if let Err(e) = result {
        tracing::warn!(error = ?e, action, "failed to write audit record");
    }
}
