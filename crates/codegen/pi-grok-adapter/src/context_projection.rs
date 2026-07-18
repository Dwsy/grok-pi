use crate::model::{PiModel, number, string};
use serde_json::{Value, json};

/// Extract context-window used tokens from Pi `get_session_stats` data.
pub(crate) fn context_tokens_from_stats(data: &Value) -> Option<u64> {
    let usage = data.get("contextUsage")?;
    // After compaction Pi may report null until the next assistant response.
    usage
        .get("tokens")
        .and_then(Value::as_u64)
        .or_else(|| {
            usage
                .get("tokens")
                .and_then(Value::as_f64)
                .map(|value| value.round() as u64)
        })
        .filter(|&tokens| tokens > 0)
}

fn context_window_from_stats(data: &Value) -> Option<u64> {
    data.get("contextUsage").and_then(|usage| {
        usage
            .get("contextWindow")
            .or_else(|| usage.get("context_window"))
            .and_then(|value| {
                value
                    .as_u64()
                    .or_else(|| value.as_f64().map(|n| n.round() as u64))
            })
            .filter(|&tokens| tokens > 0)
    })
}

fn usage_pct_from_stats(data: &Value, used: u64, total: u64) -> u8 {
    if let Some(percent) = data
        .get("contextUsage")
        .and_then(|usage| usage.get("percent"))
        .and_then(Value::as_f64)
    {
        return percent.round().clamp(0.0, 100.0) as u8;
    }
    if total == 0 {
        0
    } else {
        ((used as f64 / total as f64) * 100.0)
            .round()
            .clamp(0.0, 100.0) as u8
    }
}

/// Approximate token count used by the pi-context extension (`ceil(len/4)`).
fn estimate_tokens_text(text: &str) -> u64 {
    if text.is_empty() {
        0
    } else {
        ((text.len() as f64) / 4.0).ceil() as u64
    }
}

fn estimate_tokens_value(value: &Value) -> u64 {
    match value {
        Value::Null => 0,
        Value::String(text) => estimate_tokens_text(text),
        Value::Array(items) => items.iter().map(estimate_tokens_value).sum(),
        Value::Object(map) => map.values().map(estimate_tokens_value).sum(),
        other => estimate_tokens_text(&other.to_string()),
    }
}

/// Project `get_entries` payload into a `{ messages: [...] }` shape that
/// [`estimate_message_tokens`] already understands.
pub(crate) fn entries_to_messages_value(entries: Value) -> Value {
    let items = entries
        .get("entries")
        .and_then(Value::as_array)
        .cloned()
        .or_else(|| entries.as_array().cloned())
        .unwrap_or_default();
    let messages: Vec<Value> = items
        .into_iter()
        .filter_map(|entry| {
            let kind = string(&entry, &["type", "kind"])
                .unwrap_or_default()
                .to_ascii_lowercase();
            if kind == "message" || kind.is_empty() {
                Some(entry.get("message").cloned().unwrap_or(entry))
            } else if kind.contains("compaction") || kind.contains("branch") {
                // Keep summary text so compaction/branch tokens contribute.
                Some(entry)
            } else {
                None
            }
        })
        .collect();
    json!({ "messages": messages })
}

/// Raw (unscaled) message-window estimate, mirroring pi-context categories.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct MessageTokenEstimate {
    /// User + assistant text (+ thought) content.
    messages: u64,
    /// Tool calls + tool results (shown as Reasoning/overhead when unallocated).
    tool_payload: u64,
    compaction_count: u64,
}

fn estimate_message_tokens(messages: &Value) -> MessageTokenEstimate {
    let mut estimate = MessageTokenEstimate::default();
    let Some(items) = messages
        .get("messages")
        .and_then(Value::as_array)
        .or_else(|| messages.as_array())
    else {
        return estimate;
    };
    for item in items {
        let message = item.get("message").unwrap_or(item);
        let role = string(message, &["role", "type"])
            .unwrap_or_default()
            .to_ascii_lowercase();
        match role.as_str() {
            "user" => {
                estimate.messages +=
                    estimate_tokens_value(message.get("content").unwrap_or(message));
            }
            "assistant" => {
                if let Some(content) = message.get("content").and_then(Value::as_array) {
                    for part in content {
                        match content_kind_local(part).as_str() {
                            "text" => {
                                if let Some(text) = string(part, &["text"]) {
                                    estimate.messages += estimate_tokens_text(text);
                                }
                            }
                            "thinking" | "reasoning" => {
                                if let Some(text) = string(part, &["thinking", "reasoning", "text"])
                                {
                                    estimate.messages += estimate_tokens_text(text);
                                }
                            }
                            "toolcall" | "tool_call" | "tool" => {
                                estimate.tool_payload += estimate_tokens_value(part);
                            }
                            _ => {
                                estimate.messages += estimate_tokens_value(part);
                            }
                        }
                    }
                } else if let Some(text) = message.get("content").and_then(Value::as_str) {
                    estimate.messages += estimate_tokens_text(text);
                }
            }
            "toolresult" | "tool_result" => {
                estimate.tool_payload +=
                    estimate_tokens_value(message.get("content").unwrap_or(&Value::Null));
            }
            "bashexecution" | "bash_execution" => {
                estimate.tool_payload +=
                    estimate_tokens_text(string(message, &["command"]).unwrap_or_default());
                estimate.tool_payload +=
                    estimate_tokens_text(string(message, &["output"]).unwrap_or_default());
            }
            "branchsummary" | "branch_summary" | "compactionsummary" | "compaction_summary" => {
                estimate.messages +=
                    estimate_tokens_text(string(message, &["summary", "text"]).unwrap_or_default());
                if role.contains("compaction") {
                    estimate.compaction_count += 1;
                }
            }
            _ => {}
        }
    }
    estimate
}

fn content_kind_local(value: &Value) -> String {
    string(value, &["type", "kind"])
        .unwrap_or_default()
        .to_ascii_lowercase()
        .replace(['-', '_'], "")
}

/// Scale raw char-based estimates so they sum to the authoritative `used`
/// total from Pi (`contextUsage.tokens`), same ratio trick as pi-context.
fn scale_token_parts(parts: &[u64], used: u64) -> Vec<u64> {
    let raw_total: u64 = parts.iter().sum();
    if raw_total == 0 || used == 0 {
        return parts.iter().map(|_| 0).collect();
    }
    let ratio = used as f64 / raw_total as f64;
    let mut scaled: Vec<u64> = parts
        .iter()
        .map(|&part| ((part as f64) * ratio).round() as u64)
        .collect();
    // Keep the bar total coherent: adjust the largest bucket if rounding drifts.
    let scaled_sum: u64 = scaled.iter().sum();
    if scaled_sum != used
        && let Some((idx, _)) = scaled.iter().enumerate().max_by_key(|(_, value)| *value)
    {
        if scaled_sum > used {
            scaled[idx] = scaled[idx].saturating_sub(scaled_sum - used);
        } else {
            scaled[idx] = scaled[idx].saturating_add(used - scaled_sum);
        }
    }
    scaled
}

/// Project Pi stats (+ optional message estimate) into Grok `SessionInfoResponse` JSON.
///
/// Shape matches `xai_grok_shell::session::SessionInfoResponse` so the pager can
/// deserialize into `ContextInfo` and push `RenderBlock::context_info`.
pub(crate) fn build_session_info_response(
    stats: &Value,
    messages: Option<&Value>,
    session_id: &str,
    cwd: &str,
    model: Option<&PiModel>,
    cached_tokens: Option<u64>,
) -> Value {
    let used = context_tokens_from_stats(stats)
        .or(cached_tokens)
        .unwrap_or(0);
    let total = context_window_from_stats(stats)
        .or_else(|| model.and_then(|m| m.context_window))
        .unwrap_or(0);
    let estimate = messages.map(estimate_message_tokens).unwrap_or_default();
    // When message estimation fails (RPC error / empty transcript), put the
    // whole window into Messages so the bar is not 100% "Reasoning/overhead".
    let raw_parts = [estimate.messages, estimate.tool_payload];
    let raw_total: u64 = raw_parts.iter().sum();
    let message_tokens = if used == 0 {
        0
    } else if raw_total == 0 {
        used
    } else {
        scale_token_parts(&raw_parts, used)
            .first()
            .copied()
            .unwrap_or(used)
    };
    // tool_payload becomes Reasoning/overhead via used - system - messages.
    // System prompt / tool definitions are not exposed over Pi RPC, so they stay 0
    // (pi-context can read them in-process; we cannot without expanding RPC).
    let free_tokens = total.saturating_sub(used);
    let usage_pct = usage_pct_from_stats(stats, used, total);
    let turns = number(stats, &["userMessages", "user_messages"]).unwrap_or(0);
    let tool_call_count = number(stats, &["toolCalls", "tool_calls"]).unwrap_or(0);
    let message_count = number(stats, &["totalMessages", "total_messages"]).unwrap_or(0);
    let model_id = model.map(|m| m.id.clone());
    let model_label = model.map(|m| {
        if m.label.is_empty() {
            m.id.clone()
        } else {
            m.label.clone()
        }
    });

    // Build without explicit JSON nulls so Option fields that lack
    // `#[serde(default)]` never see `null` on the wire.
    let mut response = json!({
        "sessionId": session_id,
        "cwd": cwd,
        "agentName": "pi",
        "showModelFingerprint": false,
        "turns": turns,
        "turnIndex": turns.saturating_sub(1),
        "context": {
            "used": used,
            "total": total,
            "systemPromptTokens": 0,
            "toolDefinitionsCount": 0,
            "toolDefinitionsTokens": 0,
            "compactionCount": estimate.compaction_count,
            "turnCount": turns,
            "toolCallCount": tool_call_count,
            "messageCount": message_count,
            "messageTokens": message_tokens,
            "freeTokens": free_tokens,
            "usagePct": usage_pct,
            "autoCompactThresholdPercent": 85_u8,
        }
    });
    if let Some(obj) = response.as_object_mut() {
        if let Some(id) = model_id {
            obj.insert("model".into(), Value::String(id));
        }
        if let Some(label) = model_label {
            obj.insert("modelDisplayName".into(), Value::String(label));
        }
    }
    response
}

/// Approximate context tokens from a Pi assistant `usage` object.
///
/// Mirrors Pi's `calculateContextTokens`: prefer `totalTokens`, else sum
/// input/output/cache components.
pub(crate) fn context_tokens_from_usage(usage: &Value) -> Option<u64> {
    if let Some(total) = usage
        .get("totalTokens")
        .or_else(|| usage.get("total_tokens"))
        .and_then(Value::as_u64)
        .filter(|&tokens| tokens > 0)
    {
        return Some(total);
    }
    let field = |names: &[&str]| -> u64 {
        names
            .iter()
            .find_map(|name| usage.get(*name).and_then(Value::as_u64))
            .unwrap_or(0)
    };
    let total = field(&["input"])
        + field(&["output"])
        + field(&["cacheRead", "cache_read"])
        + field(&["cacheWrite", "cache_write"]);
    (total > 0).then_some(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_tokens_prefer_session_stats_and_usage_total() {
        assert_eq!(
            context_tokens_from_stats(&json!({
                "contextUsage": { "tokens": 60_000, "contextWindow": 200_000, "percent": 30.0 }
            })),
            Some(60_000)
        );
        assert_eq!(
            context_tokens_from_stats(&json!({
                "contextUsage": { "tokens": null, "contextWindow": 200_000, "percent": null }
            })),
            None
        );
        assert_eq!(
            context_tokens_from_usage(&json!({
                "totalTokens": 12_345,
                "input": 1,
                "output": 2,
            })),
            Some(12_345)
        );
        assert_eq!(
            context_tokens_from_usage(&json!({
                "input": 100,
                "output": 50,
                "cacheRead": 20,
                "cacheWrite": 5,
            })),
            Some(175)
        );
    }

    #[test]
    fn session_info_maps_pi_stats_into_native_context_shape() {
        let model = PiModel {
            provider: "openai".into(),
            id: "gpt-test".into(),
            label: "GPT Test".into(),
            context_window: Some(200_000),
            max_tokens: None,
            api: None,
            base_url: None,
            reasoning: false,
            accepts_images: false,
            input: Vec::new(),
            cost_input: None,
            cost_output: None,
            cost_cache_read: None,
            cost_cache_write: None,
            thinking_levels: vec!["off".into()],
        };
        let stats = json!({
            "sessionId": "sess-1",
            "userMessages": 3,
            "assistantMessages": 2,
            "toolCalls": 4,
            "toolResults": 4,
            "totalMessages": 9,
            "contextUsage": { "tokens": 10_000, "contextWindow": 200_000, "percent": 5.0 }
        });
        let messages = json!({
            "messages": [
                { "role": "user", "content": "hello world" },
                {
                    "role": "assistant",
                    "content": [
                        { "type": "text", "text": "thinking out loud" },
                        {
                            "type": "toolCall",
                            "id": "t1",
                            "name": "read",
                            "arguments": { "path": "a.rs" }
                        }
                    ]
                },
                {
                    "role": "toolResult",
                    "toolCallId": "t1",
                    "content": [{ "type": "text", "text": "fn main() {}" }]
                }
            ]
        });
        let response = build_session_info_response(
            &stats,
            Some(&messages),
            "sess-1",
            "/repo",
            Some(&model),
            None,
        );
        assert_eq!(response["sessionId"], json!("sess-1"));
        assert_eq!(response["cwd"], json!("/repo"));
        assert_eq!(response["model"], json!("gpt-test"));
        assert_eq!(response["modelDisplayName"], json!("GPT Test"));
        assert_eq!(response["agentName"], json!("pi"));
        assert_eq!(response["turns"], json!(3));
        assert_eq!(response["context"]["used"], json!(10_000));
        assert_eq!(response["context"]["total"], json!(200_000));
        assert_eq!(response["context"]["freeTokens"], json!(190_000));
        assert_eq!(response["context"]["usagePct"], json!(5));
        assert_eq!(response["context"]["toolCallCount"], json!(4));
        assert_eq!(response["context"]["messageCount"], json!(9));
        assert_eq!(response["context"]["turnCount"], json!(3));
        // Message bucket must be non-zero when text is present and scales into used.
        assert!(response["context"]["messageTokens"].as_u64().unwrap_or(0) > 0);
        let message_tokens = response["context"]["messageTokens"].as_u64().unwrap();
        assert!(message_tokens <= 10_000);
        // system stays 0 without RPC access; overhead fills the rest of used.
        assert_eq!(response["context"]["systemPromptTokens"], json!(0));
    }

    #[test]
    fn session_info_falls_back_to_cached_tokens_when_stats_null() {
        let stats = json!({
            "userMessages": 1,
            "toolCalls": 0,
            "totalMessages": 1,
            "contextUsage": { "tokens": null, "contextWindow": 100_000, "percent": null }
        });
        let response =
            build_session_info_response(&stats, None, "sess-2", "/tmp", None, Some(42_000));
        assert_eq!(response["context"]["used"], json!(42_000));
        assert_eq!(response["context"]["total"], json!(100_000));
        assert_eq!(response["context"]["freeTokens"], json!(58_000));
        // No message estimate → put the full used window into Messages.
        assert_eq!(response["context"]["messageTokens"], json!(42_000));
        assert!(response.get("model").is_none());
        assert!(response.get("resolvedModelId").is_none());
    }

    #[test]
    fn entries_payload_projects_message_roles() {
        let entries = json!({
            "entries": [
                { "type": "message", "message": { "role": "user", "content": "hi there" } },
                {
                    "type": "message",
                    "message": {
                        "role": "assistant",
                        "content": [{ "type": "text", "text": "hello" }]
                    }
                }
            ]
        });
        let messages = entries_to_messages_value(entries);
        let estimate = estimate_message_tokens(&messages);
        assert!(estimate.messages > 0);
    }

    #[test]
    fn scale_token_parts_preserves_used_total() {
        assert_eq!(scale_token_parts(&[75, 25], 1_000), vec![750, 250]);
        let scaled = scale_token_parts(&[1, 1, 1], 10);
        assert_eq!(scaled.iter().sum::<u64>(), 10);
    }
}
