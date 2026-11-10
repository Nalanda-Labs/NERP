//! Stage time limits. When a record enters a stage with a limit, a check is queued for the
//! moment the limit runs out; if the record is still there (and has not left and come
//! back since), it is flagged in its timeline and its responsible person is told — with
//! escalation, their department heads too.

use chrono::{DateTime, Duration, Utc};
use serde_json::{json, Value};
use uuid::Uuid;

use super::SYSTEM_USER;
use crate::{
    crm::{broadcast, entity_type_by_id, timeline},
    error::AppResult,
    jobs,
    notify::{notify, NewNotification},
    state::AppState,
};

pub const JOB: &str = "crm.sla";

pub async fn stage_entered(state: &AppState, item: Uuid, stage: Uuid) -> AppResult<()> {
    let limit: Option<i32> = sqlx::query_scalar("SELECT sla_minutes FROM crm_stages WHERE id = $1").bind(stage).fetch_optional(&state.db).await?.flatten();
    let Some(minutes) = limit else { return Ok(()) };
    let since: Option<DateTime<Utc>> = sqlx::query_scalar("SELECT stage_changed_at FROM crm_items WHERE id = $1 AND stage_id = $2").bind(item).bind(stage).fetch_optional(&state.db).await?;
    let Some(since) = since else { return Ok(()) };
    jobs::enqueue(&state.db, JOB, json!({ "item_id": item, "stage_id": stage, "since": since }), Some(since + Duration::minutes(minutes as i64))).await?;
    Ok(())
}

/// Who hears about a record over its limit: the responsible person, plus with escalation
/// the heads of their departments (or of the parent departments, for a head).
async fn recipients(state: &AppState, assigned: Uuid, escalate: bool) -> AppResult<Vec<Uuid>> {
    let mut to = vec![assigned];
    if escalate {
        let heads: Vec<Uuid> = sqlx::query_scalar::<_, Option<Uuid>>(
            "SELECT DISTINCT CASE WHEN d.head_user_id = $1 THEN p.head_user_id ELSE d.head_user_id END
               FROM user_departments ud
               JOIN departments d ON d.id = ud.department_id
               LEFT JOIN departments p ON p.id = d.parent_id
              WHERE ud.user_id = $1",
        )
        .bind(assigned)
        .fetch_all(&state.db)
        .await?
        .into_iter()
        .flatten()
        .collect();
        for h in heads {
            if !to.contains(&h) {
                to.push(h);
            }
        }
    }
    Ok(to)
}

pub fn duration_text(minutes: i64) -> String {
    let (d, h, m) = (minutes / 1440, minutes % 1440 / 60, minutes % 60);
    match (d, h, m) {
        (0, 0, m) => format!("{m} min"),
        (0, h, 0) => format!("{h} h"),
        (0, h, m) => format!("{h} h {m} min"),
        (d, 0, _) => format!("{d} d"),
        (d, h, _) => format!("{d} d {h} h"),
    }
}

pub async fn run_job(state: &AppState, payload: &Value) -> anyhow::Result<()> {
    let item: Uuid = serde_json::from_value(payload["item_id"].clone())?;
    let stage: Uuid = serde_json::from_value(payload["stage_id"].clone())?;
    let since: DateTime<Utc> = serde_json::from_value(payload["since"].clone())?;
    let row: Option<(Option<Uuid>, DateTime<Utc>, Option<Uuid>, String, i16)> =
        sqlx::query_as("SELECT stage_id, stage_changed_at, assigned_to, title, entity_type FROM crm_items WHERE id = $1 AND deleted_at IS NULL")
            .bind(item)
            .fetch_optional(&state.db)
            .await?;
    let Some((current, changed, assigned, title, et)) = row else { return Ok(()) };
    if current != Some(stage) || (changed - since).num_milliseconds().abs() > 1 {
        return Ok(()); // moved on (or left and came back: that visit has its own check)
    }
    let s: Option<(String, Option<i32>, bool)> = sqlx::query_as("SELECT name, sla_minutes, sla_escalate FROM crm_stages WHERE id = $1").bind(stage).fetch_optional(&state.db).await?;
    let Some((name, Some(minutes), escalate)) = s else { return Ok(()) }; // limit removed meanwhile
    // The limit may have been shortened or lengthened since the check was queued.
    let due = since + Duration::minutes(minutes as i64);
    if due > Utc::now() + Duration::seconds(5) {
        jobs::enqueue(&state.db, JOB, payload.clone(), Some(due)).await?;
        return Ok(());
    }
    let ae = |e: crate::error::AppError| anyhow::anyhow!("{e:?}");
    let limit = duration_text(minutes as i64);
    timeline(&state.db, item, "sla", Some(SYSTEM_USER), None, json!({ "stage": { "id": stage, "name": name }, "minutes": minutes })).await?;
    let t = entity_type_by_id(state, et).await.map_err(ae)?;
    if let Some(a) = assigned {
        let to = recipients(state, a, escalate).await.map_err(ae)?;
        notify(state, &to, NewNotification {
            kind: "crm.sla",
            title: format!("Over the time limit: {title}"),
            body: Some(format!("{} has been in “{name}” for more than {limit}", t.name)),
            link: Some(format!("/crm/{}/{item}", t.code)),
            actor: Some(SYSTEM_USER),
        })
        .await;
    }
    broadcast(state, et, item, false).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn durations() {
        assert_eq!(super::duration_text(45), "45 min");
        assert_eq!(super::duration_text(120), "2 h");
        assert_eq!(super::duration_text(150), "2 h 30 min");
        assert_eq!(super::duration_text(2880), "2 d");
        assert_eq!(super::duration_text(3000), "2 d 2 h");
    }
}
