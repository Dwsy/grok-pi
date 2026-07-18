//! Project Pi background-Bash custom messages into existing Grok task updates.

use serde_json::{Value, json};

const BRIDGE_TYPE: &str = "pi-grok-background-bash/v1";

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum BackgroundBashProjection {
    Started {
        task_id: String,
        tool_call_id: String,
        command: String,
        cwd: String,
        output_file: String,
        description: Option<String>,
    },
    Completed {
        tool_call_id: String,
        task_snapshot: Value,
    },
}

/// Parse a Pi `message_end` custom message emitted by the private grok-pi Bash
/// extension. Unknown custom messages intentionally return `None` so they keep
/// their normal Pi message handling.
pub(crate) fn parse_background_bash_message(event: &Value) -> Option<BackgroundBashProjection> {
    let message = event.get("message").unwrap_or(event);
    if field_str(message, "role") != Some("custom")
        || field_str(message, "customType") != Some(BRIDGE_TYPE)
    {
        return None;
    }
    let details = message.get("details").unwrap_or(message);
    match field_str(details, "event")? {
        "started" => Some(BackgroundBashProjection::Started {
            task_id: required_string(details, "taskId")?,
            tool_call_id: required_string(details, "toolCallId")?,
            command: required_string(details, "command")?,
            cwd: required_string(details, "cwd")?,
            output_file: required_string(details, "outputFile")?,
            description: optional_string(details, "description"),
        }),
        "completed" => {
            let task_snapshot = details.get("taskSnapshot")?.clone();
            if task_snapshot
                .get("task_id")
                .and_then(Value::as_str)
                .is_none()
            {
                return None;
            }
            Some(BackgroundBashProjection::Completed {
                tool_call_id: required_string(details, "toolCallId")?,
                task_snapshot,
            })
        }
        _ => None,
    }
}

/// Extract the immediate background-task registration from the private Bash
/// tool result. Tool lifecycle events are emitted synchronously, unlike a
/// custom message sent after a child process completes while Pi is streaming.
pub(crate) fn parse_background_bash_tool_result(
    tool_name: &str,
    tool_call_id: &str,
    _args: Option<&Value>,
    result: &Value,
) -> Option<BackgroundBashProjection> {
    if tool_name != "bash" {
        return None;
    }
    let details = result.get("details")?;
    if details.get("background").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    Some(BackgroundBashProjection::Started {
        task_id: required_string(details, "taskId")?,
        tool_call_id: tool_call_id.to_string(),
        command: required_string(details, "command")?,
        cwd: required_string(details, "cwd")?,
        output_file: required_string(details, "outputFile")?,
        description: optional_string(details, "description"),
    })
}

/// Render a projection as the exact session-notification envelope consumed by
/// the existing Pager background-task handlers.
pub(crate) fn background_bash_notification(
    session_id: &str,
    projection: &BackgroundBashProjection,
) -> (&'static str, Value) {
    match projection {
        BackgroundBashProjection::Started {
            task_id,
            tool_call_id,
            command,
            cwd,
            output_file,
            description,
        } => (
            "x.ai/task_backgrounded",
            json!({
                "sessionId": session_id,
                "update": {
                    "sessionUpdate": "task_backgrounded",
                    "tool_call_id": tool_call_id,
                    "task_id": task_id,
                    "command": command,
                    "cwd": cwd,
                    "output_file": output_file,
                    "description": description,
                }
            }),
        ),
        BackgroundBashProjection::Completed { task_snapshot, .. } => (
            "x.ai/task_completed",
            json!({
                "sessionId": session_id,
                "update": {
                    "sessionUpdate": "task_completed",
                    "task_snapshot": task_snapshot,
                    "will_wake": false,
                }
            }),
        ),
    }
}

fn field_str<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

fn required_string(value: &Value, key: &str) -> Option<String> {
    field_str(value, key)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn optional_string(value: &Value, key: &str) -> Option<String> {
    required_string(value, key)
}

/// Build the cumulative Bash output update consumed by Pager's existing
/// background-task stdout router before the terminal completion notification.
pub(crate) fn background_bash_output_update(
    projection: &BackgroundBashProjection,
) -> Option<Value> {
    let BackgroundBashProjection::Completed {
        tool_call_id,
        task_snapshot,
    } = projection
    else {
        return None;
    };
    Some(json!({
        "toolCallId": tool_call_id,
        "rawOutput": {
            "type": "Bash",
            "output_for_prompt": task_snapshot.get("output").and_then(Value::as_str).unwrap_or(""),
            "truncated": task_snapshot.get("truncated").and_then(Value::as_bool).unwrap_or(false),
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_started_task() {
        let event = json!({
            "message": {
                "role": "custom",
                "customType": BRIDGE_TYPE,
                "details": {
                    "event": "started",
                    "taskId": "bash-1",
                    "toolCallId": "call-1",
                    "command": "cargo test",
                    "cwd": "/repo",
                    "outputFile": "/tmp/task.log",
                    "description": "Run tests",
                }
            }
        });
        assert_eq!(
            parse_background_bash_message(&event),
            Some(BackgroundBashProjection::Started {
                task_id: "bash-1".into(),
                tool_call_id: "call-1".into(),
                command: "cargo test".into(),
                cwd: "/repo".into(),
                output_file: "/tmp/task.log".into(),
                description: Some("Run tests".into()),
            })
        );
    }

    #[test]
    fn parses_background_tool_result_for_initial_or_promoted_bash() {
        let result = json!({
            "details": {
                "background": true,
                "taskId": "bash-1",
                "command": "cargo test",
                "cwd": "/repo",
                "outputFile": "/tmp/task.log",
            }
        });
        assert_eq!(
            parse_background_bash_tool_result("bash", "call-1", None, &result),
            Some(BackgroundBashProjection::Started {
                task_id: "bash-1".into(),
                tool_call_id: "call-1".into(),
                command: "cargo test".into(),
                cwd: "/repo".into(),
                output_file: "/tmp/task.log".into(),
                description: None,
            })
        );
    }

    #[test]
    fn parses_completed_task() {
        let event = json!({
            "message": {
                "role": "custom",
                "customType": BRIDGE_TYPE,
                "details": {
                    "event": "completed",
                    "toolCallId": "call-1",
                    "taskSnapshot": { "task_id": "bash-1", "completed": true }
                }
            }
        });
        assert_eq!(
            parse_background_bash_message(&event),
            Some(BackgroundBashProjection::Completed {
                tool_call_id: "call-1".into(),
                task_snapshot: json!({ "task_id": "bash-1", "completed": true }),
            })
        );
    }

    #[test]
    fn ignores_non_bridge_messages() {
        assert!(
            parse_background_bash_message(&json!({
                "message": { "role": "custom", "customType": "pi-grok-recap/v1" }
            }))
            .is_none()
        );
    }

    #[test]
    fn builds_pager_task_notifications() {
        let (method, started) = background_bash_notification(
            "session-1",
            &BackgroundBashProjection::Started {
                task_id: "bash-1".into(),
                tool_call_id: "call-1".into(),
                command: "cargo test".into(),
                cwd: "/repo".into(),
                output_file: "/tmp/task.log".into(),
                description: None,
            },
        );
        assert_eq!(method, "x.ai/task_backgrounded");
        assert_eq!(started["sessionId"], "session-1");
        assert_eq!(started["update"]["sessionUpdate"], "task_backgrounded");
        assert_eq!(started["update"]["task_id"], "bash-1");

        let (method, completed) = background_bash_notification(
            "session-1",
            &BackgroundBashProjection::Completed {
                tool_call_id: "call-1".into(),
                task_snapshot: json!({ "task_id": "bash-1", "completed": true }),
            },
        );
        assert_eq!(method, "x.ai/task_completed");
        assert_eq!(completed["update"]["sessionUpdate"], "task_completed");
        assert_eq!(completed["update"]["will_wake"], false);
    }

    #[test]
    fn builds_cumulative_output_update_before_completion() {
        let update = background_bash_output_update(&BackgroundBashProjection::Completed {
            tool_call_id: "call-1".into(),
            task_snapshot: json!({
                "task_id": "bash-1",
                "output": "test output\n",
                "truncated": true,
            }),
        })
        .expect("completed projection creates output update");
        assert_eq!(update["toolCallId"], "call-1");
        assert_eq!(update["rawOutput"]["type"], "Bash");
        assert_eq!(update["rawOutput"]["output_for_prompt"], "test output\n");
        assert_eq!(update["rawOutput"]["truncated"], true);
    }
}
