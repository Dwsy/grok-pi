//! Project Pi custom `pi-grok-btw/v1` messages into ACP x.ai/btw answers.

use serde_json::{Value, json};

const BRIDGE_TYPE: &str = "pi-grok-btw/v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BtwProjection {
    pub request_id: String,
    pub result: Result<String, String>,
    pub model_used: Option<String>,
}

/// Parse a Pi custom message into a btw projection.
///
/// Returns `None` when the event is not a btw bridge message.
pub(crate) fn parse_btw_message(event: &Value) -> Option<BtwProjection> {
    let message = event
        .get("message")
        .or_else(|| event.get("entry").and_then(|e| e.get("message")))
        .unwrap_or(event);
    let custom_type = message
        .get("customType")
        .or_else(|| message.get("custom_type"))
        .and_then(Value::as_str)?;
    if custom_type != BRIDGE_TYPE {
        return None;
    }
    let details = message.get("details").unwrap_or(&Value::Null);
    let request_id = details
        .get("requestId")
        .or_else(|| details.get("request_id"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let model_used = details
        .get("modelUsed")
        .or_else(|| details.get("model_used"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let ok = details.get("ok").and_then(Value::as_bool).unwrap_or(false);
    if ok {
        let answer = details
            .get("answer")
            .and_then(Value::as_str)
            .or_else(|| message.get("content").and_then(Value::as_str))
            .unwrap_or("")
            .to_string();
        if answer.is_empty() {
            return Some(BtwProjection {
                request_id,
                result: Err("Empty side question response".into()),
                model_used,
            });
        }
        Some(BtwProjection {
            request_id,
            result: Ok(answer),
            model_used,
        })
    } else {
        let error = details
            .get("error")
            .and_then(Value::as_str)
            .or_else(|| message.get("content").and_then(Value::as_str))
            .unwrap_or("side question failed")
            .to_string();
        Some(BtwProjection {
            request_id,
            result: Err(error),
            model_used,
        })
    }
}

#[allow(dead_code)]
pub(crate) fn btw_answer_payload(projection: &BtwProjection) -> Value {
    match &projection.result {
        Ok(answer) => {
            let mut body = json!({ "answer": answer });
            if let Some(model) = &projection.model_used {
                body["modelUsed"] = json!(model);
            }
            body
        }
        Err(error) => json!({ "error": error }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_success() {
        let event = json!({
            "message": {
                "role": "custom",
                "customType": "pi-grok-btw/v1",
                "content": "42",
                "details": {
                    "ok": true,
                    "requestId": "r1",
                    "answer": "42",
                    "modelUsed": "openai::gpt"
                }
            }
        });
        let p = parse_btw_message(&event).expect("projection");
        assert_eq!(p.request_id, "r1");
        assert_eq!(p.result, Ok("42".into()));
        assert_eq!(p.model_used.as_deref(), Some("openai::gpt"));
    }

    #[test]
    fn parses_error() {
        let event = json!({
            "message": {
                "customType": "pi-grok-btw/v1",
                "details": {
                    "ok": false,
                    "requestId": "r2",
                    "error": "All /btw models failed"
                }
            }
        });
        let p = parse_btw_message(&event).expect("projection");
        assert_eq!(p.request_id, "r2");
        assert!(p.result.unwrap_err().contains("failed"));
    }

    #[test]
    fn ignores_other_types() {
        let event = json!({ "message": { "customType": "pi-grok-recap/v1" } });
        assert!(parse_btw_message(&event).is_none());
    }
}
