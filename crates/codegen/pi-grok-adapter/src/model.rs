use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    fs::{self, File},
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
};

/// Local Pi session metadata derived from the JSONL format owned by Pi.
///
/// This mirrors the fields Pi's `SessionManager.listAll()` uses for its native
/// selector. The adapter reads metadata only; session switching remains an RPC
/// operation executed by the Pi process.
#[derive(Debug, Clone, PartialEq)]
pub struct PiSessionInfo {
    pub path: PathBuf,
    pub id: String,
    pub cwd: String,
    pub name: Option<String>,
    pub created_at: String,
    pub modified_at: String,
    pub message_count: usize,
    pub first_message: String,
    pub model_id: Option<String>,
    pub total_tokens: Option<u64>,
    pub total_cost: Option<f64>,
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

/// One flattened Pi session-tree row for Grok's native tree surface.
///
/// Tree ownership stays in Pi (`get_tree` / `navigateTree`); this is only a
/// display projection of `{entry, children, label?, labelTimestamp?}`.
///
/// `depth` is the structural parent-chain length from the root (not the
/// visual indent). Visual indent/connectors are recomputed in the Pager
/// using Pi TreeSelector rules after filtering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PiTreeRow {
    pub id: String,
    pub parent_id: Option<String>,
    pub depth: usize,
    pub is_leaf: bool,
    pub is_current: bool,
    pub on_active_path: bool,
    pub role: String,
    pub preview: String,
    /// Longer body for detail pane (still truncated server-side).
    pub detail: String,
    pub label: Option<String>,
    pub label_timestamp: Option<String>,
    pub entry_type: String,
    pub timestamp: Option<String>,
    pub child_ids: Vec<String>,
    /// True when an assistant message has non-empty text content.
    /// Pi's default filter hides tool-only assistants (`hasText == false`).
    pub has_text: bool,
}

/// Parsed Pi `get_tree` response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PiSessionTree {
    pub leaf_id: Option<String>,
    pub rows: Vec<PiTreeRow>,
}

pub fn parse_session_tree(value: &Value) -> PiSessionTree {
    let leaf_id = string(value, &["leafId", "leaf_id"]).map(str::to_owned);
    let tree = value.get("tree").and_then(Value::as_array);
    let mut rows = Vec::new();
    if let Some(roots) = tree {
        for root in roots {
            flatten_tree_node(root, 0, None, leaf_id.as_deref(), &mut rows);
        }
    }
    // Mark the path from leaf to roots (Pi highlights active branch).
    // Guard against cycles so a corrupt parent chain cannot hang /tree forever.
    if let Some(leaf) = leaf_id.as_deref() {
        let parents: HashMap<String, Option<String>> = rows
            .iter()
            .map(|row| (row.id.clone(), row.parent_id.clone()))
            .collect();
        let mut cursor = Some(leaf.to_string());
        let mut seen = HashSet::new();
        while let Some(id) = cursor {
            if !seen.insert(id.clone()) {
                break;
            }
            if let Some(row) = rows.iter_mut().find(|row| row.id == id) {
                row.on_active_path = true;
            }
            cursor = parents.get(&id).cloned().flatten();
        }
    }
    PiSessionTree { leaf_id, rows }
}

fn flatten_tree_node(
    root: &Value,
    depth: usize,
    parent_id: Option<&str>,
    leaf_id: Option<&str>,
    out: &mut Vec<PiTreeRow>,
) {
    // Iterative DFS: deep sessions must not blow the stack or hang the adapter.
    // toolCallMap is filled while walking assistant messages so toolResult
    // rows can show `[bash: cmd]` like Pi TreeSelector, not bare `[bash]`.
    let mut stack: Vec<(Value, usize, Option<String>)> =
        vec![(root.clone(), depth, parent_id.map(str::to_owned))];
    let mut visiting = HashSet::new();
    let mut tool_calls: HashMap<String, (String, Value)> = HashMap::new();
    while let Some((node, depth, parent_id)) = stack.pop() {
        let entry = node.get("entry").cloned().unwrap_or_else(|| node.clone());
        let Some(id) = string(&entry, &["id"]).map(str::to_owned) else {
            continue;
        };
        if !visiting.insert(id.clone()) {
            // Cycle / duplicate id in tree payload.
            continue;
        }
        let entry_type = string(&entry, &["type"]).unwrap_or("unknown").to_string();
        if entry_type == "message" {
            collect_tool_calls(entry.get("message").unwrap_or(&entry), &mut tool_calls);
        }
        let label = string(&node, &["label"])
            .or_else(|| string(&entry, &["label"]))
            .map(str::to_owned);
        let label_timestamp =
            string(&node, &["labelTimestamp", "label_timestamp"]).map(str::to_owned);
        let timestamp = string(&entry, &["timestamp"]).map(str::to_owned);
        let (role, preview, detail, has_text) =
            tree_entry_display(&entry, &entry_type, &tool_calls);
        let children = node
            .get("children")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let child_ids: Vec<String> = children
            .iter()
            .filter_map(|child| {
                let entry = child.get("entry").unwrap_or(child);
                string(entry, &["id"]).map(str::to_owned)
            })
            .collect();
        let is_current = leaf_id == Some(id.as_str());
        out.push(PiTreeRow {
            id: id.clone(),
            parent_id,
            depth,
            is_leaf: children.is_empty(),
            is_current,
            on_active_path: false,
            role,
            preview,
            detail,
            label,
            label_timestamp,
            entry_type,
            timestamp,
            child_ids,
            has_text,
        });
        // Push children in reverse so left-to-right order is preserved.
        for child in children.into_iter().rev() {
            stack.push((child, depth + 1, Some(id.clone())));
        }
    }
}

fn collect_tool_calls(message: &Value, out: &mut HashMap<String, (String, Value)>) {
    let Some(parts) = message.get("content").and_then(Value::as_array) else {
        return;
    };
    for part in parts {
        if part.get("type").and_then(Value::as_str) != Some("toolCall") {
            continue;
        }
        let Some(id) = string(part, &["id"]).map(str::to_owned) else {
            continue;
        };
        let name = string(part, &["name"]).unwrap_or("tool").to_string();
        let args = part
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| Value::Object(Default::default()));
        out.insert(id, (name, args));
    }
}

/// Returns `(role, one-line preview, short detail, has_text)`.
///
/// Keep this deliberately small: Pi `get_tree` already returns full entries
/// (often multi-MB). Shipping full message bodies over ACP freezes the
/// SessionTree loading UI. List/detail use short projections; full text can
/// be re-fetched later if needed.
fn tree_entry_display(
    entry: &Value,
    entry_type: &str,
    tool_calls: &HashMap<String, (String, Value)>,
) -> (String, String, String, bool) {
    match entry_type {
        "message" => {
            let message = entry.get("message").unwrap_or(entry);
            let role = string(message, &["role"]).unwrap_or("message");
            let stop_reason = string(message, &["stopReason"]);
            let error_message = string(message, &["errorMessage"]).unwrap_or("");
            let (body, has_text) = match role {
                "user" => {
                    let text = first_text_preview(message.get("content"));
                    let has = !text.trim().is_empty();
                    (text, has)
                }
                "assistant" => {
                    let text = first_text_preview(message.get("content"));
                    let has = !text.trim().is_empty();
                    let body = if has {
                        text
                    } else if stop_reason == Some("aborted") {
                        "(aborted)".into()
                    } else if !error_message.is_empty() {
                        truncate_chars(error_message, 80)
                    } else {
                        String::new()
                    };
                    // Pi always shows current leaf; has_text drives default hide.
                    (body, has)
                }
                "toolResult" => {
                    let tool_call_id = string(message, &["toolCallId"]);
                    let body = if let Some(id) = tool_call_id {
                        if let Some((name, args)) = tool_calls.get(id) {
                            format_tool_call(name, args)
                        } else {
                            let name = string(message, &["toolName", "name"]).unwrap_or("tool");
                            format!("[{name}]")
                        }
                    } else {
                        let name = string(message, &["toolName", "name"]).unwrap_or("tool");
                        format!("[{name}]")
                    };
                    (body, false)
                }
                "bashExecution" => {
                    let command = string(message, &["command"]).unwrap_or("");
                    (format!("[bash]: {command}"), false)
                }
                other => (format!("[{other}]"), false),
            };
            let preview = if body.is_empty() && role == "assistant" {
                "(no content)".into()
            } else {
                normalize_preview(&body)
            };
            let detail = if body.is_empty() {
                preview.clone()
            } else {
                truncate_chars(body.trim(), 280)
            };
            (role.to_string(), preview, detail, has_text)
        }
        "custom_message" => {
            let custom = string(entry, &["customType"]).unwrap_or("custom");
            let body = first_text_preview(entry.get("content"));
            (
                custom.to_string(),
                normalize_preview(&body),
                truncate_chars(body.trim(), 280),
                !body.trim().is_empty(),
            )
        }
        "compaction" => {
            let tokens = entry
                .get("tokensBefore")
                .and_then(Value::as_f64)
                .map(|tokens| (tokens / 1000.0).round() as i64)
                .unwrap_or(0);
            let preview = format!("[compaction: {tokens}k tokens]");
            let summary = string(entry, &["summary"]).unwrap_or("");
            let detail = if summary.is_empty() {
                preview.clone()
            } else {
                format!("{preview}\n{}", truncate_chars(summary, 200))
            };
            ("compaction".into(), preview, detail, true)
        }
        "branch_summary" => {
            let summary = string(entry, &["summary"]).unwrap_or("");
            (
                "branch".into(),
                normalize_preview(&format!("[branch summary]: {summary}")),
                truncate_chars(summary, 280),
                true,
            )
        }
        "model_change" => {
            let text = format!(
                "[model: {}]",
                string(entry, &["modelId"]).unwrap_or("unknown")
            );
            ("model".into(), text.clone(), text, false)
        }
        "thinking_level_change" => {
            let text = format!(
                "[thinking: {}]",
                string(entry, &["thinkingLevel"]).unwrap_or("?")
            );
            ("thinking".into(), text.clone(), text, false)
        }
        "custom" => {
            let custom = string(entry, &["customType"]).unwrap_or("?");
            let preview = format!("[custom: {custom}]");
            ("custom".into(), preview.clone(), preview, false)
        }
        "label" => {
            let text = format!("[label: {}]", string(entry, &["label"]).unwrap_or(""));
            ("label".into(), text.clone(), text, false)
        }
        "session_info" => {
            let text = format!("[session: {}]", string(entry, &["name"]).unwrap_or(""));
            ("session".into(), text.clone(), text, false)
        }
        other => {
            let text = format!("[{other}]");
            (other.to_string(), text.clone(), text, false)
        }
    }
}

fn format_tool_call(name: &str, args: &Value) -> String {
    let arg = |keys: &[&str]| -> String {
        for key in keys {
            if let Some(v) = args.get(*key).and_then(Value::as_str) {
                return v.to_string();
            }
        }
        String::new()
    };
    let shorten_path = |p: &str| -> String {
        if let Ok(home) = std::env::var("HOME") {
            if !home.is_empty() && p.starts_with(&home) {
                return format!("~{}", &p[home.len()..]);
            }
        }
        p.to_string()
    };
    match name {
        "read" => {
            let path = shorten_path(&arg(&["path", "file_path"]));
            let offset = args.get("offset").and_then(Value::as_u64);
            let limit = args.get("limit").and_then(Value::as_u64);
            let mut display = path;
            if offset.is_some() || limit.is_some() {
                let start = offset.unwrap_or(1);
                display.push_str(&format!(":{start}"));
                if let Some(limit) = limit {
                    display.push_str(&format!("-{}", start + limit - 1));
                }
            }
            format!("[read: {display}]")
        }
        "write" => format!("[write: {}]", shorten_path(&arg(&["path", "file_path"]))),
        "edit" => format!("[edit: {}]", shorten_path(&arg(&["path", "file_path"]))),
        "bash" => {
            let raw = arg(&["command"]);
            let cmd = normalize_preview(&raw);
            let clipped = truncate_chars(&cmd, 50);
            if raw.chars().count() > 50 {
                format!("[bash: {clipped}...]")
            } else {
                format!("[bash: {clipped}]")
            }
        }
        "grep" => {
            let pattern = arg(&["pattern"]);
            let path = shorten_path(&arg(&["path"]));
            let path = if path.is_empty() { ".".into() } else { path };
            format!("[grep: /{pattern}/ in {path}]")
        }
        "find" => {
            let pattern = arg(&["pattern"]);
            let path = shorten_path(&arg(&["path"]));
            let path = if path.is_empty() { ".".into() } else { path };
            format!("[find: {pattern} in {path}]")
        }
        "ls" => {
            let path = shorten_path(&arg(&["path"]));
            let path = if path.is_empty() { ".".into() } else { path };
            format!("[ls: {path}]")
        }
        other => {
            let args_str = serde_json::to_string(args).unwrap_or_else(|_| "{}".into());
            let clipped = truncate_chars(&args_str, 40);
            if args_str.len() > 40 {
                format!("[{other}: {clipped}...]")
            } else {
                format!("[{other}: {clipped}]")
            }
        }
    }
}

/// Return the complete text Pi restores into its editor after tree navigation.
/// Unlike the tree projection, this is called only for the selected entry.
pub fn tree_entry_editor_text(value: &Value, entry_id: &str) -> Option<String> {
    let roots = value.get("tree")?.as_array()?;
    let mut stack: Vec<&Value> = roots.iter().rev().collect();
    while let Some(node) = stack.pop() {
        let entry = node.get("entry").unwrap_or(node);
        if string(entry, &["id"]) == Some(entry_id) {
            return match string(entry, &["type"]) {
                Some("message") => {
                    let message = entry.get("message").unwrap_or(entry);
                    (string(message, &["role"]) == Some("user"))
                        .then(|| full_text_content(message.get("content")))
                        .flatten()
                }
                Some("custom_message") => full_text_content(entry.get("content")),
                _ => None,
            };
        }
        if let Some(children) = node.get("children").and_then(Value::as_array) {
            stack.extend(children.iter().rev());
        }
    }
    None
}

fn full_text_content(content: Option<&Value>) -> Option<String> {
    match content {
        Some(Value::String(text)) => Some(text.clone()),
        Some(Value::Array(parts)) => Some(
            parts
                .iter()
                .filter(|part| part.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|part| string(part, &["text"]))
                .collect(),
        ),
        _ => None,
    }
}

fn first_text_preview(content: Option<&Value>) -> String {
    // Stop early: tree projection only needs a short preview. Reading full
    // multi-MB assistant/tool payloads is what made /tree hang on large sessions.
    const BUDGET: usize = 320;
    match content {
        Some(Value::String(text)) => truncate_chars(text, BUDGET),
        Some(Value::Array(parts)) => {
            let mut out = String::new();
            for part in parts {
                let piece = if part.get("type").and_then(Value::as_str) == Some("text") {
                    string(part, &["text"]).unwrap_or("")
                } else if part.get("type").and_then(Value::as_str) == Some("thinking") {
                    // Skip thinking blobs for tree preview.
                    ""
                } else {
                    ""
                };
                if piece.is_empty() {
                    continue;
                }
                if !out.is_empty() {
                    out.push(' ');
                }
                for ch in piece.chars() {
                    if out.chars().count() >= BUDGET {
                        out.push('…');
                        return out;
                    }
                    out.push(ch);
                }
            }
            out
        }
        _ => String::new(),
    }
}

fn normalize_preview(text: &str) -> String {
    let collapsed = text
        .chars()
        .map(|ch| if ch.is_whitespace() { ' ' } else { ch })
        .collect::<String>();
    let trimmed = collapsed.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_chars(&trimmed, 80)
}

fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out = String::new();
    for (index, ch) in text.chars().enumerate() {
        if index + 1 >= max {
            out.push('…');
            break;
        }
        out.push(ch);
    }
    out
}

/// Scan one Pi session storage directory, matching `SessionManager.listAll()`.
/// Default storage contains one project directory per CWD, while a custom
/// `--session-dir` stores JSONL files directly in its root.
pub fn scan_local_sessions(session_dir: &Path) -> Vec<PiSessionInfo> {
    scan_session_paths(session_paths(session_dir, true))
}

/// Scan only the sessions belonging to `cwd`, matching `SessionManager.list()`.
///
/// The default Pi store encodes each CWD as a child directory, so the common
/// path reads only that directory. A custom session directory stores all JSONL
/// files in one root and therefore requires filtering parsed headers by CWD.
pub fn scan_local_sessions_for_cwd(session_dir: &Path, cwd: &Path) -> Vec<PiSessionInfo> {
    let project_dir = session_dir.join(default_session_dir_name(cwd));
    let mut sessions = if project_dir.is_dir() {
        scan_session_paths(session_paths(&project_dir, false))
    } else {
        scan_session_paths(session_paths(session_dir, false))
            .into_iter()
            .filter(|session| session.cwd == cwd.to_string_lossy())
            .collect()
    };
    sessions.sort_by(|left, right| right.modified_at.cmp(&left.modified_at));
    sessions
}

fn default_session_dir_name(cwd: &Path) -> String {
    let cwd = cwd.to_string_lossy();
    let path = cwd.trim_start_matches(['/', '\\']);
    format!("--{}--", path.replace(['/', '\\', ':'], "-"))
}

fn session_paths(session_dir: &Path, include_project_dirs: bool) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(session_dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .flat_map(|entry| {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) == Some("jsonl") {
                vec![path]
            } else if include_project_dirs
                && entry.file_type().ok().is_some_and(|kind| kind.is_dir())
            {
                session_paths(&path, false)
            } else {
                Vec::new()
            }
        })
        .collect()
}

fn scan_session_paths(paths: Vec<PathBuf>) -> Vec<PiSessionInfo> {
    let mut sessions = paths
        .into_iter()
        .filter_map(|path| parse_session_file(&path))
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
    let mut model_id = None;
    let mut total_tokens = None;
    let mut total_cost = None;
    // Mirror Pi SessionManager.buildSessionInfo: max activity over user/assistant
    // messages, preferring message.timestamp (ms) then entry.timestamp (ISO).
    let mut last_activity_ms: Option<i64> = None;

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
                string(&value, &["timestamp"])
                    .unwrap_or_default()
                    .to_owned(),
            ));
            continue;
        }
        if kind == "session_info" {
            name = string(&value, &["name"])
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned);
        }
        if kind == "model_change" {
            model_id = session_model_id(&value).or(model_id);
            continue;
        }
        if kind != "message" {
            continue;
        }
        message_count += 1;
        let Some(message) = value.get("message") else {
            continue;
        };
        if let Some(ms) = message_activity_time_ms(&value, message) {
            last_activity_ms = Some(last_activity_ms.map_or(ms, |prev| prev.max(ms)));
        }
        let role = string(message, &["role"]).unwrap_or_default();
        if role == "user" && first_message.is_none() {
            first_message = session_message_text(message);
        }
        if role == "assistant" {
            model_id = session_model_id(message).or(model_id);
            if let Some(usage) = message.get("usage") {
                if let Some(tokens) = usage.get("totalTokens").and_then(Value::as_u64) {
                    total_tokens = Some(total_tokens.unwrap_or(0u64).saturating_add(tokens));
                }
                if let Some(cost) = usage
                    .get("cost")
                    .and_then(|cost| cost.get("total"))
                    .and_then(Value::as_f64)
                {
                    total_cost = Some(total_cost.unwrap_or(0.0f64) + cost);
                }
            }
        }
    }

    let (id, cwd, created_at) = header?;
    let modified_at = last_activity_ms
        .and_then(format_timestamp_ms)
        .unwrap_or_else(|| created_at.clone());
    Some(PiSessionInfo {
        path: path.to_path_buf(),
        id,
        cwd,
        name,
        modified_at,
        created_at,
        message_count,
        first_message: first_message.unwrap_or_else(|| "(no messages)".to_owned()),
        model_id,
        total_tokens,
        total_cost,
    })
}

fn session_model_id(value: &Value) -> Option<String> {
    let model_id = string(value, &["modelId", "model"])?;
    match string(value, &["provider"]) {
        Some(provider) if !provider.is_empty() => Some(format!("{provider}::{model_id}")),
        _ => Some(model_id.to_owned()),
    }
}

/// Match Pi `getMessageActivityTime`: only user/assistant messages contribute.
/// Prefer numeric `message.timestamp` (epoch ms); else parse entry-level ISO.
fn message_activity_time_ms(entry: &Value, message: &Value) -> Option<i64> {
    let role = string(message, &["role"]).unwrap_or_default();
    if !matches!(role, "user" | "assistant") {
        return None;
    }
    if let Some(ms) = message.get("timestamp").and_then(Value::as_i64) {
        return Some(ms);
    }
    if let Some(ms) = message
        .get("timestamp")
        .and_then(Value::as_f64)
        .map(|value| value as i64)
    {
        return Some(ms);
    }
    entry
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(parse_timestamp_ms)
}

fn parse_timestamp_ms(value: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|dt| dt.timestamp_millis())
}

fn format_timestamp_ms(ms: i64) -> Option<String> {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|dt| dt.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
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
    /// Pi RPC `get_state.isCompacting` — true while compaction is in flight.
    pub is_compacting: bool,
}

#[derive(Debug, Clone, Default)]
pub struct PiModel {
    pub provider: String,
    pub id: String,
    pub label: String,
    pub context_window: Option<u64>,
    pub max_tokens: Option<u64>,
    pub api: Option<String>,
    pub base_url: Option<String>,
    pub reasoning: bool,
    pub accepts_images: bool,
    /// Input modalities from Pi (`text`, `image`, …).
    pub input: Vec<String>,
    pub cost_input: Option<f64>,
    pub cost_output: Option<f64>,
    pub cost_cache_read: Option<f64>,
    pub cost_cache_write: Option<f64>,
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
            "none" | "off" => {
                if self.supports_thinking_level("off") {
                    "off"
                } else {
                    "low"
                }
            }
            "minimal" => {
                if self.supports_thinking_level("minimal") {
                    "minimal"
                } else {
                    "low"
                }
            }
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

/// A history item paired with its original message timestamp (epoch ms) from
/// the Pi session file. During replay the pager uses this to stamp the real
/// creation time on scrollback entries instead of the resume wall-clock time.
#[derive(Debug, Clone, PartialEq)]
pub struct PiReplayEntry {
    pub item: PiHistoryItem,
    /// Original message timestamp in epoch milliseconds, if available.
    pub timestamp_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PiToolContent {
    Text(String),
    Image { data: String, mime_type: String },
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
            .or_else(|| value.get("is_streaming"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        is_compacting: value
            .get("isCompacting")
            .or_else(|| value.get("is_compacting"))
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
            input: Vec::new(),
            thinking_levels: Vec::new(),
            ..PiModel::default()
        }),
        _ => {}
    }
}

pub fn parse_model(value: &Value) -> Option<PiModel> {
    let id = string(value, &["id", "modelId", "model_id"])?;
    // `api` is the protocol (openai-completions / anthropic-messages / …),
    // not the provider id — keep them separate so the picker can show both.
    let provider = string(value, &["provider", "providerId", "provider_id"]).unwrap_or_default();
    let api = string(value, &["api", "protocol"]).map(ToOwned::to_owned);
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
        &["contextWindow", "context_window", "contextWindowTokens"],
    );
    let max_tokens = number(value, &["maxTokens", "max_tokens", "maxOutputTokens"]);
    let base_url = string(value, &["baseUrl", "base_url"]).map(ToOwned::to_owned);
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
    let input = value
        .get("input")
        .or_else(|| value.get("inputModalities"))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let accepts_images = if !input.is_empty() {
        input.iter().any(|m| m.eq_ignore_ascii_case("image"))
    } else {
        value
            .get("supportsImages")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    };
    let cost = value.get("cost").and_then(Value::as_object);
    let cost_input = cost.and_then(|c| c.get("input")).and_then(Value::as_f64);
    let cost_output = cost.and_then(|c| c.get("output")).and_then(Value::as_f64);
    let cost_cache_read = cost
        .and_then(|c| c.get("cacheRead").or_else(|| c.get("cache_read")))
        .and_then(Value::as_f64);
    let cost_cache_write = cost
        .and_then(|c| c.get("cacheWrite").or_else(|| c.get("cache_write")))
        .and_then(Value::as_f64);
    let thinking_levels = supported_thinking_levels(value, reasoning);
    Some(PiModel {
        provider: provider.to_string(),
        id: id.to_string(),
        label,
        context_window,
        max_tokens,
        api,
        base_url,
        reasoning,
        accepts_images,
        input,
        cost_input,
        cost_output,
        cost_cache_read,
        cost_cache_write,
        thinking_levels,
    })
}

fn supported_thinking_levels(value: &Value, reasoning: bool) -> Vec<String> {
    if !reasoning {
        return vec!["off".to_string()];
    }
    let map = value.get("thinkingLevelMap").and_then(Value::as_object);
    let mut levels = Vec::new();
    // Pi accepts `off` only when the provider/model maps it. Reasoning models
    // commonly expose low..max without an off level; advertising off causes a
    // provider 400 when the Pager initializes or switches effort.
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
    if map.is_none() {
        levels.retain(|level| level != "off");
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

pub fn parse_messages(value: &Value) -> Vec<PiReplayEntry> {
    let source = value
        .get("messages")
        .or_else(|| value.get("history"))
        .unwrap_or(value);
    let mut history = Vec::new();
    for (message_index, message) in source.as_array().into_iter().flatten().enumerate() {
        let timestamp_ms = extract_message_timestamp(message);
        let mut items = Vec::new();
        parse_message(message, message_index, &mut items);
        for item in items {
            history.push(PiReplayEntry {
                item,
                timestamp_ms,
            });
        }
    }
    history
}

/// Extract the original timestamp (epoch ms) from a Pi session message entry.
///
/// Prefers numeric `message.timestamp` (epoch ms); falls back to entry-level
/// ISO `timestamp` string. Mirrors the logic in `message_activity_time_ms`.
fn extract_message_timestamp(entry: &Value) -> Option<i64> {
    let message = entry.get("message").unwrap_or(entry);
    // Numeric epoch-ms timestamp on the message object.
    if let Some(ms) = message.get("timestamp").and_then(Value::as_i64) {
        return Some(ms);
    }
    if let Some(ms) = message
        .get("timestamp")
        .and_then(Value::as_f64)
        .map(|v| v as i64)
    {
        return Some(ms);
    }
    // Entry-level ISO timestamp string.
    entry
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(|s| {
            chrono::DateTime::parse_from_rfc3339(s)
                .ok()
                .map(|dt| dt.timestamp_millis())
        })
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
    fn parse_session_tree_flattens_depth_and_marks_current_leaf() {
        let tree = parse_session_tree(&json!({
            "leafId": "a2",
            "tree": [{
                "entry": {
                    "type": "message",
                    "id": "u1",
                    "message": { "role": "user", "content": "first\nline" }
                },
                "children": [{
                    "entry": {
                        "type": "message",
                        "id": "a2",
                        "message": {
                            "role": "assistant",
                            "content": [{ "type": "text", "text": "reply" }]
                        }
                    },
                    "children": [],
                    "label": "checkpoint"
                }]
            }]
        }));
        assert_eq!(tree.leaf_id.as_deref(), Some("a2"));
        assert_eq!(tree.rows.len(), 2);
        assert_eq!(tree.rows[0].id, "u1");
        assert_eq!(tree.rows[0].depth, 0);
        assert_eq!(tree.rows[0].role, "user");
        assert_eq!(tree.rows[0].preview, "first line");
        assert!(!tree.rows[0].is_current);
        assert!(tree.rows[0].on_active_path);
        assert_eq!(tree.rows[0].child_ids, vec!["a2".to_string()]);
        assert_eq!(tree.rows[1].id, "a2");
        assert_eq!(tree.rows[1].parent_id.as_deref(), Some("u1"));
        assert_eq!(tree.rows[1].depth, 1);
        assert!(tree.rows[1].is_current);
        assert!(tree.rows[1].on_active_path);
        assert_eq!(tree.rows[1].label.as_deref(), Some("checkpoint"));
        assert_eq!(tree.rows[1].role, "assistant");
        assert_eq!(tree.rows[1].preview, "reply");
        assert_eq!(tree.rows[1].detail, "reply");
        assert!(tree.rows[1].has_text);
    }

    #[test]
    fn tree_entry_editor_text_preserves_full_user_message() {
        let tree = json!({
            "tree": [{
                "entry": {
                    "type": "message",
                    "id": "u1",
                    "message": {
                        "role": "user",
                        "content": [
                            { "type": "text", "text": "first line\n" },
                            { "type": "image", "data": "ignored" },
                            { "type": "text", "text": "second line" }
                        ]
                    }
                },
                "children": [{
                    "entry": {
                        "type": "custom_message",
                        "id": "c1",
                        "content": "custom text"
                    },
                    "children": []
                }]
            }]
        });

        assert_eq!(
            tree_entry_editor_text(&tree, "u1").as_deref(),
            Some("first line\nsecond line")
        );
        assert_eq!(
            tree_entry_editor_text(&tree, "c1").as_deref(),
            Some("custom text")
        );
        assert_eq!(tree_entry_editor_text(&tree, "missing"), None);
    }

    #[test]
    fn parse_session_tree_formats_tool_results_from_tool_calls() {
        let tree = parse_session_tree(&json!({
            "leafId": "tr1",
            "tree": [{
                "entry": {
                    "type": "message",
                    "id": "a1",
                    "message": {
                        "role": "assistant",
                        "content": [{
                            "type": "toolCall",
                            "id": "call-1",
                            "name": "bash",
                            "arguments": { "command": "echo hi" }
                        }]
                    }
                },
                "children": [{
                    "entry": {
                        "type": "message",
                        "id": "tr1",
                        "message": {
                            "role": "toolResult",
                            "toolCallId": "call-1",
                            "toolName": "bash",
                            "content": [{ "type": "text", "text": "hi" }]
                        }
                    },
                    "children": []
                }]
            }]
        }));
        assert_eq!(tree.rows.len(), 2);
        assert!(!tree.rows[0].has_text);
        assert_eq!(tree.rows[0].preview, "(no content)");
        assert_eq!(tree.rows[1].preview, "[bash: echo hi]");
    }

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
        assert!(matches!(items[0].item, PiHistoryItem::UserText(ref text) if text == "hello"));
        assert!(matches!(items[1].item, PiHistoryItem::AgentThought(ref text) if text == "plan"));
        assert!(matches!(items[2].item, PiHistoryItem::ToolStart { ref id, .. } if id == "tool-1"));
        assert!(matches!(items[3].item, PiHistoryItem::AgentText(ref text) if text == "done"));
        assert!(matches!(items[4].item, PiHistoryItem::ToolEnd { ref id, .. } if id == "tool-1"));
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
                "{\"type\":\"model_change\",\"id\":\"2\",\"parentId\":\"1\",\"timestamp\":\"2026-07-01T00:00:02.000Z\",\"provider\":\"openai\",\"modelId\":\"gpt-test\"}\n",
                "{\"type\":\"message\",\"id\":\"3\",\"parentId\":\"2\",\"timestamp\":\"2026-07-01T00:00:03.000Z\",\"message\":{\"role\":\"assistant\",\"provider\":\"openai\",\"model\":\"gpt-test\",\"usage\":{\"totalTokens\":1200,\"cost\":{\"total\":0.42}}}}\n",
                "{\"type\":\"session_info\",\"id\":\"4\",\"parentId\":\"3\",\"timestamp\":\"2026-07-01T00:00:04.000Z\",\"name\":\"Named session\"}\n"
            ),
        )
        .unwrap();
        std::fs::write(project.join("invalid.jsonl"), "not json\n").unwrap();

        let sessions = scan_local_sessions(&root.path().join("sessions"));
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, "session-1");
        assert_eq!(sessions[0].cwd, "/repo");
        assert_eq!(sessions[0].name.as_deref(), Some("Named session"));
        assert_eq!(sessions[0].message_count, 2);
        assert_eq!(sessions[0].first_message, "hello");
        assert_eq!(sessions[0].model_id.as_deref(), Some("openai::gpt-test"));
        assert_eq!(sessions[0].total_tokens, Some(1200));
        assert_eq!(sessions[0].total_cost, Some(0.42));
        // Entry-level ISO becomes modified_at when message.timestamp is absent.
        assert_eq!(sessions[0].created_at, "2026-07-01T00:00:00.000Z");
        assert_eq!(sessions[0].modified_at, "2026-07-01T00:00:03.000Z");
    }

    #[test]
    fn session_modified_at_prefers_message_timestamp_ms_as_rfc3339() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("sessions/project");
        std::fs::create_dir_all(&project).unwrap();
        // 2026-07-01T00:00:02.000Z == 1782864002000 ms
        std::fs::write(
            project.join("session.jsonl"),
            concat!(
                "{\"type\":\"session\",\"id\":\"session-ms\",\"timestamp\":\"2026-07-01T00:00:00.000Z\",\"cwd\":\"/repo\"}\n",
                "{\"type\":\"message\",\"id\":\"1\",\"parentId\":null,\"timestamp\":\"2026-07-01T00:00:01.000Z\",\"message\":{\"role\":\"user\",\"content\":\"hello\",\"timestamp\":1782864002000}}\n"
            ),
        )
        .unwrap();

        let sessions = scan_local_sessions(&root.path().join("sessions"));
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].modified_at, "2026-07-01T00:00:02.000Z");
        // Must be parseable RFC3339, never a bare millis digit string.
        assert!(
            sessions[0]
                .modified_at
                .parse::<chrono::DateTime<chrono::Utc>>()
                .is_ok()
        );
        assert!(!sessions[0].modified_at.chars().all(|c| c.is_ascii_digit()));
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
    fn scans_only_current_cwd_from_default_session_store() {
        let root = tempfile::tempdir().unwrap();
        let sessions = root.path().join("sessions");
        let current_cwd = Path::new("/workspace/current");
        let current_dir = sessions.join("--workspace-current--");
        let other_dir = sessions.join("--workspace-other--");
        std::fs::create_dir_all(&current_dir).unwrap();
        std::fs::create_dir_all(&other_dir).unwrap();
        std::fs::write(
            current_dir.join("current.jsonl"),
            "{\"type\":\"session\",\"id\":\"current\",\"timestamp\":\"2026-07-01T00:00:00.000Z\",\"cwd\":\"/workspace/current\"}\n",
        )
        .unwrap();
        std::fs::write(
            other_dir.join("other.jsonl"),
            "{\"type\":\"session\",\"id\":\"other\",\"timestamp\":\"2026-07-01T00:00:00.000Z\",\"cwd\":\"/workspace/other\"}\n",
        )
        .unwrap();

        let current = scan_local_sessions_for_cwd(&sessions, current_cwd);
        assert_eq!(current.len(), 1);
        assert_eq!(current[0].id, "current");
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
        assert!(
            matches!(items.as_slice(), [PiReplayEntry { item: PiHistoryItem::AgentText(text), .. }] if text == "**Pi error:** request failed")
        );
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

    #[test]
    fn parse_model_keeps_provider_and_api_separate_with_cost() {
        let model = parse_model(&json!({
            "id": "claude-haiku-4-5",
            "name": "Claude Haiku 4.5",
            "provider": "anthropic",
            "api": "anthropic-messages",
            "baseUrl": "https://api.anthropic.com",
            "contextWindow": 200000,
            "maxTokens": 64000,
            "reasoning": true,
            "input": ["text", "image"],
            "cost": { "input": 1.0, "output": 5.0, "cacheRead": 0.1, "cacheWrite": 1.25 }
        }))
        .expect("model");
        assert_eq!(model.provider, "anthropic");
        assert_eq!(model.id, "claude-haiku-4-5");
        assert_eq!(model.label, "Claude Haiku 4.5");
        assert_eq!(model.api.as_deref(), Some("anthropic-messages"));
        assert_eq!(model.base_url.as_deref(), Some("https://api.anthropic.com"));
        assert_eq!(model.context_window, Some(200_000));
        assert_eq!(model.max_tokens, Some(64_000));
        assert!(model.reasoning);
        assert!(model.accepts_images);
        assert_eq!(model.input, vec!["text".to_string(), "image".to_string()]);
        assert_eq!(model.cost_input, Some(1.0));
        assert_eq!(model.cost_output, Some(5.0));
        assert_eq!(model.cost_cache_read, Some(0.1));
        assert_eq!(model.cost_cache_write, Some(1.25));
    }

    #[test]
    fn parse_state_reads_streaming_and_compacting_flags() {
        let state = parse_state(&json!({
            "sessionId": "s1",
            "isStreaming": true,
            "isCompacting": true,
            "thinkingLevel": "high",
        }));
        assert_eq!(state.session_id, "s1");
        assert!(state.is_streaming);
        assert!(state.is_compacting);
        assert_eq!(state.thinking_level, "high");

        let idle = parse_state(&json!({ "sessionId": "s2" }));
        assert!(!idle.is_streaming);
        assert!(!idle.is_compacting);
    }
}
