//! Minimal bridge for grok-pi `/loop` scheduled tasks (extension-owned timers).
//!
//! Builds pager-facing `ScheduledTask*` session notifications from extension
//! bridge payloads. Full shell `SchedulerActor` (durable / subagent) is residual.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Task snapshot from `extensions/pi-grok-loop`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LoopTask {
    pub id: String,
    pub prompt: String,
    pub human_schedule: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_fire_at: Option<String>,
}

/// Parse a pi-grok-loop/v1 bridge details object into (event, task).
pub fn parse_loop_bridge(details: &Value) -> Option<(String, LoopTask)> {
    let event = details.get("type").and_then(Value::as_str)?.to_string();
    let task_val = details.get("task")?;
    let id = task_val
        .get("id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())?
        .to_string();
    let prompt = task_val
        .get("prompt")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let human_schedule = task_val
        .get("humanSchedule")
        .and_then(Value::as_str)
        .unwrap_or("scheduled")
        .to_string();
    let next_fire_at = details
        .get("nextFireAt")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            task_val
                .get("nextFireAt")
                .and_then(Value::as_str)
                .map(str::to_string)
        });
    Some((
        event,
        LoopTask {
            id,
            prompt,
            human_schedule,
            next_fire_at,
        },
    ))
}

/// Build method + SessionNotification params for the pager.
pub fn scheduled_task_notification(
    session_id: &str,
    event: &str,
    task: &LoopTask,
) -> Option<(String, Value)> {
    let (method, update) = match event {
        "scheduled_task_created" => (
            "x.ai/scheduled_task_created",
            json!({
                "sessionUpdate": "scheduled_task_created",
                "task_id": task.id,
                "prompt": task.prompt,
                "human_schedule": task.human_schedule,
                "next_fire_at": task.next_fire_at,
            }),
        ),
        "scheduled_task_fired" => (
            "x.ai/scheduled_task_fired",
            json!({
                "sessionUpdate": "scheduled_task_fired",
                "task_id": task.id,
                "prompt": task.prompt,
                "human_schedule": task.human_schedule,
                "next_fire_at": task.next_fire_at,
            }),
        ),
        "scheduled_task_deleted" => (
            "x.ai/scheduled_task_deleted",
            json!({
                "sessionUpdate": "scheduled_task_deleted",
                "task_id": task.id,
            }),
        ),
        _ => return None,
    };
    Some((
        method.to_string(),
        json!({
            "sessionId": session_id,
            "update": update,
        }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_created_bridge() {
        let details = json!({
            "type": "scheduled_task_created",
            "task": {
                "id": "abc",
                "prompt": "check deploy",
                "humanSchedule": "every 5 minutes",
                "nextFireAt": "2026-07-22T00:00:00Z",
            }
        });
        let (event, task) = parse_loop_bridge(&details).expect("parse");
        assert_eq!(event, "scheduled_task_created");
        assert_eq!(task.id, "abc");
        assert_eq!(task.human_schedule, "every 5 minutes");
        assert_eq!(task.next_fire_at.as_deref(), Some("2026-07-22T00:00:00Z"));
    }

    #[test]
    fn notification_created_shape() {
        let task = LoopTask {
            id: "t1".into(),
            prompt: "ping".into(),
            human_schedule: "every 1 minute".into(),
            next_fire_at: Some("2026-07-22T01:00:00Z".into()),
        };
        let (method, payload) =
            scheduled_task_notification("sess", "scheduled_task_created", &task).unwrap();
        assert_eq!(method, "x.ai/scheduled_task_created");
        assert_eq!(payload["sessionId"], "sess");
        assert_eq!(payload["update"]["task_id"], "t1");
        assert_eq!(payload["update"]["sessionUpdate"], "scheduled_task_created");
    }

    #[test]
    fn notification_deleted() {
        let task = LoopTask {
            id: "t2".into(),
            prompt: "x".into(),
            human_schedule: "every 1 day".into(),
            next_fire_at: None,
        };
        let (method, payload) =
            scheduled_task_notification("s", "scheduled_task_deleted", &task).unwrap();
        assert_eq!(method, "x.ai/scheduled_task_deleted");
        assert_eq!(payload["update"]["task_id"], "t2");
    }
}
