use crate::{model::json_text, queue_bridge::QueueLane};
use agent_client_protocol as acp;
use serde_json::{Value, json};

pub(crate) fn direct_bash_command(blocks: &[acp::ContentBlock]) -> Option<String> {
    blocks.iter().find_map(|block| {
        let acp::ContentBlock::Text(text) = block else {
            return None;
        };
        text.meta
            .as_ref()
            .and_then(|meta| meta.get("bash_command"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|command| !command.is_empty())
            .map(ToOwned::to_owned)
    })
}

/// Stamp client `promptId` on PromptResponse so the pager can discard queued
/// mid-turn RPC completions that never became `current_prompt_id`.
pub(crate) fn prompt_response(
    reason: acp::StopReason,
    client_prompt_id: Option<&str>,
) -> acp::PromptResponse {
    let mut response = acp::PromptResponse::new(reason);
    if let Some(prompt_id) = client_prompt_id.filter(|id| !id.is_empty()) {
        let mut meta = acp::Meta::new();
        meta.insert("promptId".into(), Value::String(prompt_id.to_string()));
        response = response.meta(Some(meta));
    }
    response
}

pub(crate) fn prompt_streaming_behavior(
    already_active: bool,
    meta: Option<&acp::Meta>,
) -> Option<&'static str> {
    if !already_active {
        return None;
    }
    // Cancel-and-send / send-now is an interrupt (steer).
    if meta
        .and_then(|meta| meta.get("sendNow"))
        .and_then(Value::as_bool)
        == Some(true)
    {
        return Some("steer");
    }
    // Explicit followUp meta wins; otherwise mid-turn prompts queue as follow-up
    // (FEATURE_MATRIX: default active-turn prompt → Pi follow_up).
    if meta
        .and_then(|meta| meta.get("followUp"))
        .and_then(Value::as_bool)
        == Some(false)
    {
        return Some("steer");
    }
    Some("followUp")
}

pub(crate) fn queue_lane_for_behavior(behavior: &str) -> Option<QueueLane> {
    match behavior {
        "steer" => Some(QueueLane::Steering),
        "followUp" => Some(QueueLane::FollowUp),
        _ => None,
    }
}

pub(crate) fn format_bash_result(result: &Value) -> String {
    let mut text = result
        .get("output")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let mut notes = Vec::new();
    if result.get("cancelled").and_then(Value::as_bool) == Some(true) {
        notes.push("Command cancelled".to_string());
    } else if let Some(exit_code) = result.get("exitCode").and_then(Value::as_i64) {
        notes.push(format!("Exit code: {exit_code}"));
    }
    if result.get("truncated").and_then(Value::as_bool) == Some(true) {
        let suffix = result
            .get("fullOutputPath")
            .and_then(Value::as_str)
            .map(|path| format!("Output truncated; full output: {path}"))
            .unwrap_or_else(|| "Output truncated".to_string());
        notes.push(suffix);
    }
    if !notes.is_empty() {
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&notes.join("\n"));
    }
    if text.is_empty() {
        "Command completed with no output".to_string()
    } else {
        text
    }
}

pub(crate) fn prompt_to_pi(blocks: &[acp::ContentBlock]) -> (String, Vec<Value>) {
    let mut parts = Vec::new();
    let mut images = Vec::new();
    for block in blocks {
        match block {
            acp::ContentBlock::Text(text) => parts.push(text.text.clone()),
            acp::ContentBlock::Image(image) => images.push(json!({
                "type": "image",
                "data": image.data,
                "mimeType": image.mime_type,
            })),
            acp::ContentBlock::ResourceLink(link) => {
                parts.push(format!("[resource] {}", link.uri));
            }
            acp::ContentBlock::Resource(resource) => {
                parts.push(json_text(
                    &serde_json::to_value(resource).unwrap_or(Value::Null),
                ));
            }
            _ => {}
        }
    }
    (parts.join("\n\n"), images)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grok_direct_bash_meta_maps_to_pi_bash() {
        let mut meta = acp::Meta::new();
        meta.insert("bash_command".into(), json!("git status"));
        let blocks = vec![acp::ContentBlock::Text(
            acp::TextContent::new("!git status").meta(Some(meta)),
        )];
        assert_eq!(direct_bash_command(&blocks).as_deref(), Some("git status"));
    }

    #[test]
    fn prompt_response_echoes_client_prompt_id() {
        let response = prompt_response(acp::StopReason::EndTurn, Some("client-uuid"));
        assert_eq!(response.stop_reason, acp::StopReason::EndTurn);
        assert_eq!(
            response
                .meta
                .as_ref()
                .and_then(|meta| meta.get("promptId"))
                .and_then(Value::as_str),
            Some("client-uuid")
        );
        let bare = prompt_response(acp::StopReason::Cancelled, None);
        assert!(bare.meta.is_none());
    }

    #[test]
    fn pi_tui_queue_modes_map_to_pi_streaming_behavior() {
        assert_eq!(prompt_streaming_behavior(false, None), None);
        // Mid-turn default is follow-up (wait for turn), not steer.
        assert_eq!(prompt_streaming_behavior(true, None), Some("followUp"));

        let mut meta = acp::Meta::new();
        meta.insert("followUp".into(), Value::Bool(true));
        assert_eq!(
            prompt_streaming_behavior(true, Some(&meta)),
            Some("followUp")
        );

        let mut send_now = acp::Meta::new();
        send_now.insert("sendNow".into(), Value::Bool(true));
        assert_eq!(
            prompt_streaming_behavior(true, Some(&send_now)),
            Some("steer")
        );

        let mut force_steer = acp::Meta::new();
        force_steer.insert("followUp".into(), Value::Bool(false));
        assert_eq!(
            prompt_streaming_behavior(true, Some(&force_steer)),
            Some("steer")
        );
    }

    #[test]
    fn bash_result_is_presented_in_native_tool_card_text() {
        let text = format_bash_result(&json!({
            "output": "ok",
            "exitCode": 0,
            "cancelled": false,
            "truncated": true,
            "fullOutputPath": "/tmp/pi-bash.log",
        }));
        assert!(text.contains("ok"));
        assert!(text.contains("Exit code: 0"));
        assert!(text.contains("/tmp/pi-bash.log"));
    }
}
