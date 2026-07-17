use serde_json::Value;
use std::{
    fs::{self, File},
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
};

/// Local Pi session metadata derived from the JSONL format owned by Pi.
///
/// This mirrors the fields Pi's `SessionManager.listAll()` uses for its native
/// selector. The adapter reads metadata only; session switching remains an RPC
/// operation executed by the Pi process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PiSessionInfo {
    pub path: PathBuf,
    pub id: String,
    pub cwd: String,
    pub name: Option<String>,
    pub created_at: String,
    pub modified_at: String,
    pub message_count: usize,
    pub first_message: String,
}

/// Pi's `switch_session` response. A cancelled response is successful RPC
/// transport-wise but must not replace the adapter's active session metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PiSessionSwitch {
    pub cancelled: bool,
}

pub fn parse_session_switch(value: &Value) -> PiSessionSwitch {
    PiSessionSwitch {
        cancelled: value
            .get("cancelled")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    }
}

/// Scan one Pi session storage directory, matching `SessionManager.listAll()`.
/// Default storage contains one project directory per CWD, while a custom
/// `--session-dir` stores JSONL files directly in its root.
pub fn scan_local_sessions(session_dir: &Path) -> Vec<PiSessionInfo> {
    let Ok(entries) = fs::read_dir(session_dir) else {
        return Vec::new();
    };
    let mut sessions = entries
        .flatten()
        .flat_map(|entry| {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) == Some("jsonl") {
                vec![path]
            } else if entry.file_type().ok().is_some_and(|kind| kind.is_dir()) {
                fs::read_dir(path)
                    .into_iter()
                    .flatten()
                    .flatten()
                    .map(|child| child.path())
                    .collect()
            } else {
                Vec::new()
            }
        })
        .filter_map(|path| {
            (path.extension().and_then(|ext| ext.to_str()) == Some("jsonl"))
                .then(|| parse_session_file(&path))
                .flatten()
        })
        .collect::<Vec<_>>();
    sessions.sort_by(|left, right| right.modified_at.cmp(&left.modified_at));
    sessions
}

fn parse_session_file(path: &Path) -> Option<PiSessionInfo> {
    let file = File::open(path).ok()?;
    let mut header: Option<(String, String, String)> = None;
    let mut name = None;
    let mut message_count = 0;
    let mut first_message = None;
    let mut modified_at = None;

    for line in BufReader::new(file).lines().map_while(Result::ok) {
        let value: Value = serde_json::from_str(&line).ok()?;
        let kind = string(&value, &["type"])?;
        if header.is_none() {
            if kind != "session" {
                return None;
            }
            header = Some((
                string(&value, &["id"])?.to_owned(),
                string(&value, &["cwd"]).unwrap_or_default().to_owned(),
                string(&value, &["timestamp"]).unwrap_or_default().to_owned(),
            ));
            continue;
        }
        if kind == "session_info" {
            name = string(&value, &["name"])
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned);
        }
        if kind != "message" {
            continue;
        }
        message_count += 1;
        let Some(message) = value.get("message") else {
            continue;
        };
        let role = string(message, &["role"]).unwrap_or_default();
        if !matches!(role, "user" | "assistant") {
            continue;
        }
        if let Some(timestamp) = message.get("timestamp").and_then(Value::as_i64) {
            modified_at = Some(timestamp.to_string());
        }
        if role == "user" && first_message.is_none() {
            first_message = session_message_text(message);
        }
    }

    let (id, cwd, created_at) = header?;
    Some(PiSessionInfo {
        path: path.to_path_buf(),
        id,
        cwd,
        name,
        modified_at: modified_at.unwrap_or_else(|| created_at.clone()),
        created_at,
        message_count,
        first_message: first_message.unwrap_or_else(|| "(no messages)".to_owned()),
    })
}

fn session_message_text(message: &Value) -> Option<String> {
    match message.get("content")? {
        Value::String(text) if !text.trim().is_empty() => Some(text.clone()),
        Value::Array(blocks) => {
            let text = blocks
                .iter()
                .filter(|block| string(block, &["type"]) == Some("text"))
                .filter_map(|block| string(block, &["text"]))
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            (!text.is_empty()).then_some(text)
        }
        _ => None,
    }
}

#[derive(Debug, Clone, Default)]
pub struct PiState {
    pub session_id: String,
    pub session_file: Option<String>,
    pub session_name: Option<String>,
    pub model: Option<PiModel>,
    pub thinking_level: String,
    pub is_streaming: bool,
}

#[derive(Debug, Clone, Default)]
pub struct PiModel {
    pub provider: String,
    pub id: String,
    pub label: String,
    pub context_window: Option<u64>,
    pub reasoning: bool,
    pub accepts_images: bool,
    /// Pi-level tokens accepted by `set_thinking_level` for this model. This is
    /// derived with the same rules as Pi's `getSupportedThinkingLevels()`:
    /// standard levels default to enabled, `null` disables a level, and the
    /// extended `xhigh`/`max` levels are opt-in.
    pub thinking_levels: Vec<String>,
}

impl PiModel {
    pub fn supports_thinking_level(&self, level: &str) -> bool {
        self.thinking_levels
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(level))
    }

    /// Grok's native effort enum has one top slot (`xhigh`) while Pi may expose
    /// `xhigh`, `max`, or both. Prefer Pi's strongest available level so the
    /// native selector never sends an unsupported token.
    pub fn pi_level_for_acp_effort(&self, effort: &str) -> Option<&'static str> {
        let requested = match effort.to_ascii_lowercase().as_str() {
            "none" | "off" => "off",
            "minimal" => "minimal",
            "low" => "low",
            "medium" => "medium",
            "high" => "high",
            "xhigh" | "max" => {
                if self.supports_thinking_level("max") {
                    "max"
                } else {
                    "xhigh"
                }
            }
            _ => return None,
        };
        self.supports_thinking_level(requested).then_some(requested)
    }
}

#[derive(Debug, Clone, Default)]
pub struct PiCommand {
    pub name: String,
    pub description: String,
    pub source: String,
}

/// Structured Pi history projected onto ACP. Keeping tool calls and images as
/// first-class items lets the native Grok pager reuse its real markdown, image,
/// reasoning, and tool-card renderers during session replay.
#[derive(Debug, Clone, PartialEq)]
pub enum PiHistoryItem {
    UserText(String),
    UserImage {
        data: String,
        mime_type: String,
    },
    AgentText(String),
    AgentThought(String),
    ToolStart {
        id: String,
        name: String,
        arguments: Option<Value>,
    },
    ToolEnd {
        id: String,
        name: String,
        content: Vec<PiToolContent>,
        raw_output: Option<Value>,
        is_error: bool,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum PiToolContent {
    Text(String),
    Image {
        data: String,
        mime_type: String,
    },
}

pub fn parse_state(value: &Value) -> PiState {
    PiState {
        session_id: string(value, &["sessionId", "session_id"])
            .unwrap_or("pi-session")
            .to_string(),
        session_file: string(value, &["sessionFile", "session_file", "sessionPath"])
            .map(ToOwned::to_owned),
        session_name: string(value, &["sessionName", "session_name"]).map(ToOwned::to_owned),
        model: value.get("model").and_then(parse_model),
        thinking_level: string(value, &["thinkingLevel", "thinking_level"])
            .unwrap_or("medium")
            .to_string(),
        is_streaming: value
            .get("isStreaming")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    }
}

pub fn parse_models(value: &Value) -> Vec<PiModel> {
    let source = value
        .get("models")
        .or_else(|| value.get("availableModels"))
        .unwrap_or(value);
    let mut models = Vec::new();
    collect_models(source, "", &mut models);
    models.sort_by(|a, b| a.label.cmp(&b.label));
    models.dedup_by(|a, b| a.provider == b.provider && a.id == b.id);
    models
}

fn collect_models(value: &Value, provider_hint: &str, out: &mut Vec<PiModel>) {
    match value {
        Value::Array(values) => {
            for value in values {
                if let Some(mut model) = parse_model(value) {
                    if model.provider.is_empty() {
                        model.provider = provider_hint.to_string();
                    }
                    out.push(model);
                } else {
                    collect_models(value, provider_hint, out);
                }
            }
        }
        Value::Object(map) => {
            if let Some(mut model) = parse_model(value) {
                if model.provider.is_empty() {
                    model.provider = provider_hint.to_string();
                }
                out.push(model);
            } else {
                for (key, child) in map {
                    let next = if child.is_array() { key } else { provider_hint };
                    collect_models(child, next, out);
                }
            }
        }
        Value::String(id) => out.push(PiModel {
            provider: provider_hint.to_string(),
            id: id.clone(),
            label: if provider_hint.is_empty() {
                id.clone()
            } else {
                format!("{provider_hint}/{id}")
            },
            accepts_images: false,
            thinking_levels: Vec::new(),
            ..PiModel::default()
        }),
        _ => {}
    }
}

pub fn parse_model(value: &Value) -> Option<PiModel> {
    let id = string(value, &["id", "modelId", "model_id"])?;
    let provider = string(value, &["provider", "providerId", "provider_id", "api"])
        .unwrap_or_default();
    let label = string(value, &["name", "label", "displayName", "display_name"])
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| {
            if provider.is_empty() {
                id.to_string()
            } else {
                format!("{provider}/{id}")
            }
        });
    let context_window = number(
        value,
        &[
            "contextWindow",
            "context_window",
            "contextWindowTokens",
            "maxTokens",
        ],
    );
    let reasoning = value
        .get("reasoning")
        .and_then(Value::as_bool)
        .or_else(|| value.get("supportsReasoning").and_then(Value::as_bool))
        .unwrap_or_else(|| {
            value
                .get("capabilities")
                .and_then(|caps| caps.get("reasoning"))
                .and_then(Value::as_bool)
                .unwrap_or(false)
        });
    let accepts_images = value
        .get("input")
        .or_else(|| value.get("inputModalities"))
        .and_then(Value::as_array)
        .map(|items| items.iter().any(|v| v.as_str() == Some("image")))
        .or_else(|| value.get("supportsImages").and_then(Value::as_bool))
        .unwrap_or(false);
    let thinking_levels = supported_thinking_levels(value, reasoning);
    Some(PiModel {
        provider: provider.to_string(),
        id: id.to_string(),
        label,
        context_window,
        reasoning,
        accepts_images,
        thinking_levels,
    })
}

fn supported_thinking_levels(value: &Value, reasoning: bool) -> Vec<String> {
    if !reasoning {
        return vec!["off".to_string()];
    }
    let map = value.get("thinkingLevelMap").and_then(Value::as_object);
    let mut levels = Vec::new();
    for level in ["off", "minimal", "low", "medium", "high"] {
        let supported = map
            .and_then(|entries| entries.get(level))
            .map(|mapped| !mapped.is_null())
            .unwrap_or(true);
        if supported {
            levels.push(level.to_string());
        }
    }
    for level in ["xhigh", "max"] {
        let supported = map
            .and_then(|entries| entries.get(level))
            .is_some_and(|mapped| !mapped.is_null());
        if supported {
            levels.push(level.to_string());
        }
    }
    levels
}

pub fn parse_commands(value: &Value) -> Vec<PiCommand> {
    let source = value.get("commands").unwrap_or(value);
    let mut commands = Vec::new();
    if let Some(items) = source.as_array() {
        for item in items {
            let Some(name) = string(item, &["name", "command", "id"]) else {
                continue;
            };
            commands.push(PiCommand {
                name: name.trim_start_matches('/').to_string(),
                description: string(item, &["description", "help", "title"])
                    .unwrap_or_default()
                    .to_string(),
                source: string(item, &["source", "origin"])
                    .unwrap_or_default()
                    .to_string(),
            });
        }
    }
    commands.sort_by(|a, b| a.name.cmp(&b.name));
    commands.dedup_by(|a, b| a.name == b.name);
    commands
}

pub fn parse_messages(value: &Value) -> Vec<PiHistoryItem> {
    let source = value
        .get("messages")
        .or_else(|| value.get("history"))
        .unwrap_or(value);
    let mut history = Vec::new();
    for (message_index, message) in source
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
    {
        parse_message(message, message_index, &mut history);
    }
    history
}

fn parse_message(value: &Value, message_index: usize, output: &mut Vec<PiHistoryItem>) {
    let value = value.get("message").unwrap_or(value);
    let role = string(value, &["role", "type"])
        .unwrap_or_default()
        .to_ascii_lowercase();
    match role.as_str() {
        "user" => parse_user_content(value.get("content").unwrap_or(value), output),
        "assistant" => parse_assistant(value, message_index, output),
        "toolresult" | "tool_result" => parse_tool_result(value, output),
        "bashexecution" | "bash_execution" => parse_bash_execution(value, message_index, output),
        "custom" => {
            if value.get("display").and_then(Value::as_bool) != Some(false) {
                parse_agent_content(value.get("content").unwrap_or(value), output);
            }
        }
        "branchsummary" | "branch_summary" => {
            if let Some(summary) = string(value, &["summary", "text"]) {
                output.push(PiHistoryItem::AgentText(format!(
                    "**Branch summary**\n\n{summary}"
                )));
            }
        }
        "compactionsummary" | "compaction_summary" => {
            if let Some(summary) = string(value, &["summary", "text"]) {
                output.push(PiHistoryItem::AgentText(format!(
                    "**Compaction summary**\n\n{summary}"
                )));
            }
        }
        _ => {
            // Unknown extension-defined messages are only replayed when they
            // carry explicit displayable content. This avoids inventing UI for
            // opaque backend bookkeeping records.
            parse_agent_content(value.get("content").unwrap_or(value), output);
        }
    }
}

fn parse_user_content(value: &Value, output: &mut Vec<PiHistoryItem>) {
    match value {
        Value::String(text) if !text.is_empty() => {
            output.push(PiHistoryItem::UserText(text.clone()));
        }
        Value::Array(items) => {
            for item in items {
                match content_kind(item).as_str() {
                    "image" => {
                        if let Some((data, mime_type)) = image_content(item) {
                            output.push(PiHistoryItem::UserImage { data, mime_type });
                        }
                    }
                    _ => {
                        if let Some(text) = content_text(item) {
                            output.push(PiHistoryItem::UserText(text.to_string()));
                        }
                    }
                }
            }
        }
        Value::Object(_) => {
            if let Some(content) = value.get("content") {
                parse_user_content(content, output);
            } else if let Some(text) = content_text(value) {
                output.push(PiHistoryItem::UserText(text.to_string()));
            }
        }
        _ => {}
    }
}

fn parse_assistant(value: &Value, message_index: usize, output: &mut Vec<PiHistoryItem>) {
    let Some(content) = value.get("content") else {
        if let Some(text) = content_text(value) {
            output.push(PiHistoryItem::AgentText(text.to_string()));
        }
        append_assistant_error(value, output);
        return;
    };
    match content {
        Value::String(text) if !text.is_empty() => {
            output.push(PiHistoryItem::AgentText(text.clone()));
        }
        Value::Array(items) => {
            for (block_index, item) in items.iter().enumerate() {
                match content_kind(item).as_str() {
                    "thinking" | "reasoning" => {
                        if let Some(text) = string(item, &["thinking", "reasoning", "text"]) {
                            if !text.is_empty() {
                                output.push(PiHistoryItem::AgentThought(text.to_string()));
                            }
                        }
                    }
                    "toolcall" | "tool_call" | "tool" => {
                        let id = string(item, &["id", "toolCallId", "tool_call_id"])
                            .map(ToOwned::to_owned)
                            .unwrap_or_else(|| {
                                format!("pi-history-tool-{message_index}-{block_index}")
                            });
                        let name = string(item, &["name", "toolName", "tool_name"])
                            .unwrap_or("Tool")
                            .to_string();
                        let arguments = item
                            .get("arguments")
                            .or_else(|| item.get("args"))
                            .or_else(|| item.get("input"))
                            .cloned();
                        output.push(PiHistoryItem::ToolStart {
                            id,
                            name,
                            arguments,
                        });
                    }
                    _ => {
                        if let Some(text) = content_text(item) {
                            if !text.is_empty() {
                                output.push(PiHistoryItem::AgentText(text.to_string()));
                            }
                        }
                    }
                }
            }
        }
        Value::Object(_) => parse_agent_content(content, output),
        _ => {}
    }
    append_assistant_error(value, output);
}

fn append_assistant_error(value: &Value, output: &mut Vec<PiHistoryItem>) {
    if let Some(error) = string(value, &["errorMessage", "error_message"])
        && !error.is_empty()
    {
        output.push(PiHistoryItem::AgentText(format!("**Pi error:** {error}")));
    }
}

fn parse_agent_content(value: &Value, output: &mut Vec<PiHistoryItem>) {
    match value {
        Value::String(text) if !text.is_empty() => {
            output.push(PiHistoryItem::AgentText(text.clone()));
        }
        Value::Array(items) => {
            for item in items {
                if matches!(content_kind(item).as_str(), "thinking" | "reasoning") {
                    if let Some(text) = string(item, &["thinking", "reasoning", "text"]) {
                        output.push(PiHistoryItem::AgentThought(text.to_string()));
                    }
                } else if let Some(text) = content_text(item) {
                    output.push(PiHistoryItem::AgentText(text.to_string()));
                }
            }
        }
        Value::Object(_) => {
            if let Some(content) = value.get("content") {
                parse_agent_content(content, output);
            } else if let Some(text) = content_text(value) {
                output.push(PiHistoryItem::AgentText(text.to_string()));
            }
        }
        _ => {}
    }
}

fn parse_tool_result(value: &Value, output: &mut Vec<PiHistoryItem>) {
    let Some(id) = string(value, &["toolCallId", "tool_call_id", "id"]) else {
        return;
    };
    let name = string(value, &["toolName", "tool_name", "name"])
        .unwrap_or("Tool")
        .to_string();
    let mut content = Vec::new();
    if let Some(items) = value.get("content").and_then(Value::as_array) {
        for item in items {
            if content_kind(item) == "image" {
                if let Some((data, mime_type)) = image_content(item) {
                    content.push(PiToolContent::Image { data, mime_type });
                }
            } else if let Some(text) = content_text(item) {
                content.push(PiToolContent::Text(text.to_string()));
            }
        }
    } else if let Some(text) = value.get("content").and_then(Value::as_str) {
        content.push(PiToolContent::Text(text.to_string()));
    }
    let raw_output = value
        .get("details")
        .cloned()
        .or_else(|| value.get("content").cloned());
    output.push(PiHistoryItem::ToolEnd {
        id: id.to_string(),
        name,
        content,
        raw_output,
        is_error: value.get("isError").and_then(Value::as_bool) == Some(true),
    });
}

fn parse_bash_execution(value: &Value, message_index: usize, output: &mut Vec<PiHistoryItem>) {
    let id = format!("pi-history-bash-{message_index}");
    let command = string(value, &["command"]).unwrap_or_default().to_string();
    output.push(PiHistoryItem::ToolStart {
        id: id.clone(),
        name: "bash".to_string(),
        arguments: Some(serde_json::json!({ "command": command })),
    });
    let mut text = string(value, &["output"]).unwrap_or_default().to_string();
    if value.get("cancelled").and_then(Value::as_bool) == Some(true) {
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        text.push_str("Command cancelled");
    } else if let Some(code) = value.get("exitCode").and_then(Value::as_i64) {
        if code != 0 {
            if !text.is_empty() {
                text.push_str("\n\n");
            }
            text.push_str(&format!("Command exited with code {code}"));
        }
    }
    output.push(PiHistoryItem::ToolEnd {
        id,
        name: "bash".to_string(),
        content: (!text.is_empty())
            .then(|| vec![PiToolContent::Text(text)])
            .unwrap_or_default(),
        raw_output: value.get("output").cloned(),
        is_error: value.get("cancelled").and_then(Value::as_bool) == Some(true)
            || value
                .get("exitCode")
                .and_then(Value::as_i64)
                .is_some_and(|code| code != 0),
    });
}

fn content_kind(value: &Value) -> String {
    string(value, &["type", "kind"])
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
}

fn content_text(value: &Value) -> Option<&str> {
    string(value, &["text", "content", "message", "output"])
}

fn image_content(value: &Value) -> Option<(String, String)> {
    let data = string(value, &["data"])?;
    let mime_type = string(value, &["mimeType", "mime_type"])?;
    Some((data.to_string(), mime_type.to_string()))
}

pub fn extract_delta(value: &Value) -> (String, String) {
    let nested = value
        .get("assistantMessageEvent")
        .or_else(|| value.get("messageEvent"))
        .unwrap_or(value);
    let kind = string(nested, &["type", "kind"])
        .unwrap_or_default()
        .to_ascii_lowercase();
    let delta = string(
        nested,
        &["delta", "textDelta", "contentDelta", "text", "chunk"],
    )
    .unwrap_or_default()
    .to_string();
    if kind.contains("thinking") || kind.contains("reasoning") {
        (String::new(), delta)
    } else if kind.contains("text") {
        (delta, String::new())
    } else {
        (String::new(), String::new())
    }
}

pub fn string<'a>(value: &'a Value, names: &[&str]) -> Option<&'a str> {
    names
        .iter()
        .find_map(|name| value.get(*name).and_then(Value::as_str))
}

pub fn number(value: &Value, names: &[&str]) -> Option<u64> {
    names
        .iter()
        .find_map(|name| value.get(*name).and_then(Value::as_u64))
}

pub fn json_text(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        _ => serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn history_preserves_reasoning_tools_and_results() {
        let items = parse_messages(&json!({
            "messages": [
                { "role": "user", "content": "hello" },
                {
                    "role": "assistant",
                    "content": [
                        { "type": "thinking", "thinking": "plan" },
                        { "type": "toolCall", "id": "tool-1", "name": "read", "arguments": { "path": "README.md" } },
                        { "type": "text", "text": "done" }
                    ]
                },
                {
                    "role": "toolResult",
                    "toolCallId": "tool-1",
                    "toolName": "read",
                    "content": [{ "type": "text", "text": "file" }],
                    "isError": false
                }
            ]
        }));
        assert!(matches!(items[0], PiHistoryItem::UserText(ref text) if text == "hello"));
        assert!(matches!(items[1], PiHistoryItem::AgentThought(ref text) if text == "plan"));
        assert!(matches!(items[2], PiHistoryItem::ToolStart { ref id, .. } if id == "tool-1"));
        assert!(matches!(items[3], PiHistoryItem::AgentText(ref text) if text == "done"));
        assert!(matches!(items[4], PiHistoryItem::ToolEnd { ref id, .. } if id == "tool-1"));
    }

    #[test]
    fn scans_pi_session_metadata_with_native_selector_fields() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("sessions/project");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(
            project.join("session.jsonl"),
            concat!(
                "{\"type\":\"session\",\"id\":\"session-1\",\"timestamp\":\"2026-07-01T00:00:00.000Z\",\"cwd\":\"/repo\"}\n",
                "{\"type\":\"message\",\"id\":\"1\",\"parentId\":null,\"timestamp\":\"2026-07-01T00:00:01.000Z\",\"message\":{\"role\":\"user\",\"content\":\"hello\"}}\n",
                "{\"type\":\"session_info\",\"id\":\"2\",\"parentId\":\"1\",\"timestamp\":\"2026-07-01T00:00:02.000Z\",\"name\":\"Named session\"}\n"
            ),
        )
        .unwrap();
        std::fs::write(project.join("invalid.jsonl"), "not json\n").unwrap();

        let sessions = scan_local_sessions(&root.path().join("sessions"));
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, "session-1");
        assert_eq!(sessions[0].cwd, "/repo");
        assert_eq!(sessions[0].name.as_deref(), Some("Named session"));
        assert_eq!(sessions[0].message_count, 1);
        assert_eq!(sessions[0].first_message, "hello");
    }

    #[test]
    fn scans_custom_session_directory_without_default_project_nesting() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join("session.jsonl"),
            concat!(
                "{\"type\":\"session\",\"id\":\"custom-session\",\"timestamp\":\"2026-07-01T00:00:00.000Z\",\"cwd\":\"/repo\"}\n",
                "{\"type\":\"message\",\"id\":\"1\",\"parentId\":null,\"timestamp\":\"2026-07-01T00:00:01.000Z\",\"message\":{\"role\":\"user\",\"content\":\"hello\"}}\n"
            ),
        )
        .unwrap();

        let sessions = scan_local_sessions(root.path());
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, "custom-session");
    }

    #[test]
    fn parses_cancelled_session_switch_without_state_mutation_signal() {
        assert_eq!(
            parse_session_switch(&json!({ "cancelled": true })),
            PiSessionSwitch { cancelled: true }
        );
        assert_eq!(
            parse_session_switch(&json!({ "cancelled": false })),
            PiSessionSwitch { cancelled: false }
        );
    }

    #[test]
    fn assistant_errors_are_preserved_without_content() {
        let items = parse_messages(&json!({
            "messages": [{ "role": "assistant", "errorMessage": "request failed" }]
        }));
        assert!(matches!(items.as_slice(), [PiHistoryItem::AgentText(text)] if text == "**Pi error:** request failed"));
    }

    #[test]
    fn delta_parser_ignores_toolcall_stream_fragments() {
        assert_eq!(
            extract_delta(&json!({
                "assistantMessageEvent": { "type": "toolcall_delta", "delta": "{\\\"path\\\":" }
            })),
            (String::new(), String::new())
        );
    }
}
