use crate::model::{PiToolContent, json_text, string};
use agent_client_protocol as acp;
use indexmap::IndexMap;
use serde_json::{Value, json};

pub(crate) fn history_tool_content(content: Vec<PiToolContent>) -> Vec<acp::ToolCallContent> {
    content
        .into_iter()
        .map(|item| match item {
            PiToolContent::Text(text) => {
                acp::ToolCallContent::from(acp::ContentBlock::Text(acp::TextContent::new(text)))
            }
            PiToolContent::Image { data, mime_type } => acp::ToolCallContent::from(
                acp::ContentBlock::Image(acp::ImageContent::new(data, mime_type)),
            ),
        })
        .collect()
}

pub(crate) fn tool_content(value: &Value) -> Vec<acp::ToolCallContent> {
    let source = value.get("content").unwrap_or(value);
    let mut output = Vec::new();
    match source {
        Value::Array(items) => {
            for item in items {
                let kind = string(item, &["type", "kind"])
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                if kind == "image"
                    && let (Some(data), Some(mime_type)) = (
                        string(item, &["data"]),
                        string(item, &["mimeType", "mime_type"]),
                    )
                {
                    output.push(acp::ToolCallContent::from(acp::ContentBlock::Image(
                        acp::ImageContent::new(data, mime_type),
                    )));
                } else {
                    let text = string(item, &["text", "content", "message", "output"])
                        .map(ToOwned::to_owned)
                        .unwrap_or_else(|| json_text(item));
                    if !text.is_empty() {
                        output.push(acp::ToolCallContent::from(acp::ContentBlock::Text(
                            acp::TextContent::new(text),
                        )));
                    }
                }
            }
        }
        _ => {
            let text = json_text(source);
            if !text.is_empty() {
                output.push(acp::ToolCallContent::from(acp::ContentBlock::Text(
                    acp::TextContent::new(text),
                )));
            }
        }
    }
    output
}

#[derive(Clone, Copy)]
struct EditLineNumbers {
    old_line: usize,
    new_line: usize,
}

struct PatchLine {
    text: String,
    old_line: Option<usize>,
    new_line: Option<usize>,
}

struct PatchHunk {
    lines: Vec<PatchLine>,
}

/// Convert Pi's edit/write input contract into ACP's native diff payload.
///
/// The Pager's Edit card and viewer intentionally render only `Diff` content;
/// ordinary text results do not provide the old/new source needed for a hunk.
pub(crate) fn edit_diff_content(
    tool_name: &str,
    args: Option<&Value>,
    result: Option<&Value>,
) -> Option<Vec<acp::ToolCallContent>> {
    if tool_kind(tool_name) != acp::ToolKind::Edit {
        return None;
    }
    let args = args?;
    let path = string(args, &["path", "filePath", "file_path", "target_file"])?;
    let line_numbers = edit_line_numbers(args, result);
    if let Some(edits) = args.get("edits").and_then(Value::as_array) {
        let diffs = edits
            .iter()
            .enumerate()
            .filter_map(|(index, edit)| {
                let old_text = string(edit, &["oldText", "old_text"])?;
                let new_text = string(edit, &["newText", "new_text"])?;
                Some(edit_diff(
                    path,
                    Some(old_text),
                    new_text,
                    line_numbers.get(index).copied().flatten(),
                ))
            })
            .collect::<Vec<_>>();
        return (!diffs.is_empty()).then_some(diffs);
    }
    let new_text = string(args, &["newText", "new_text", "content"])?;
    let old_text = string(args, &["oldText", "old_text"]);
    Some(vec![edit_diff(
        path,
        old_text,
        new_text,
        line_numbers.first().copied().flatten(),
    )])
}

fn edit_diff(
    path: &str,
    old_text: Option<&str>,
    new_text: &str,
    line_numbers: Option<EditLineNumbers>,
) -> acp::ToolCallContent {
    let mut diff =
        acp::Diff::new(path, new_text.to_owned()).old_text(old_text.map(ToOwned::to_owned));
    if let Some(line_numbers) = line_numbers {
        let meta = json!({
            "old_line": line_numbers.old_line,
            "new_line": line_numbers.new_line,
        })
        .as_object()
        .cloned();
        diff = diff.meta(meta);
    }
    acp::ToolCallContent::Diff(diff)
}

fn edit_line_numbers(args: &Value, result: Option<&Value>) -> Vec<Option<EditLineNumbers>> {
    let Some(patch) = result
        .and_then(|result| {
            result
                .pointer("/details/patch")
                .or_else(|| result.get("patch"))
        })
        .and_then(Value::as_str)
    else {
        return Vec::new();
    };
    let hunks = parse_patch_hunks(patch);
    edit_replacements(args)
        .into_iter()
        .map(|(old_text, _)| find_edit_line_numbers(&hunks, old_text))
        .collect()
}

fn edit_replacements(args: &Value) -> Vec<(&str, &str)> {
    if let Some(edits) = args.get("edits").and_then(Value::as_array) {
        return edits
            .iter()
            .filter_map(|edit| {
                Some((
                    string(edit, &["oldText", "old_text"])?,
                    string(edit, &["newText", "new_text"])?,
                ))
            })
            .collect();
    }
    match (
        string(args, &["oldText", "old_text"]),
        string(args, &["newText", "new_text"]),
    ) {
        (Some(old_text), Some(new_text)) => vec![(old_text, new_text)],
        _ => Vec::new(),
    }
}

fn parse_patch_hunks(patch: &str) -> Vec<PatchHunk> {
    let mut hunks = Vec::new();
    let mut current: Option<(PatchHunk, usize, usize)> = None;

    for line in patch.lines() {
        if let Some((old_line, new_line)) = parse_patch_hunk_header(line) {
            if let Some((hunk, _, _)) = current.take() {
                hunks.push(hunk);
            }
            current = Some((PatchHunk { lines: Vec::new() }, old_line, new_line));
            continue;
        }
        let Some((hunk, old_line, new_line)) = current.as_mut() else {
            continue;
        };
        let Some((prefix, text)) = line.split_at_checked(1) else {
            continue;
        };
        match prefix {
            " " => {
                hunk.lines.push(PatchLine {
                    text: text.to_string(),
                    old_line: Some(*old_line),
                    new_line: Some(*new_line),
                });
                *old_line += 1;
                *new_line += 1;
            }
            "-" => {
                hunk.lines.push(PatchLine {
                    text: text.to_string(),
                    old_line: Some(*old_line),
                    new_line: None,
                });
                *old_line += 1;
            }
            "+" => {
                hunk.lines.push(PatchLine {
                    text: text.to_string(),
                    old_line: None,
                    new_line: Some(*new_line),
                });
                *new_line += 1;
            }
            _ => {}
        }
    }
    if let Some((hunk, _, _)) = current {
        hunks.push(hunk);
    }
    hunks
}

fn parse_patch_hunk_header(line: &str) -> Option<(usize, usize)> {
    let ranges = line.strip_prefix("@@ ")?.split_once(" @@")?.0;
    let mut ranges = ranges.split_whitespace();
    let old_line = parse_patch_range_start(ranges.next()?)?;
    let new_line = parse_patch_range_start(ranges.next()?)?;
    Some((old_line, new_line))
}

fn parse_patch_range_start(range: &str) -> Option<usize> {
    range
        .strip_prefix(['-', '+'])?
        .split_once(',')
        .map(|(start, _)| start)
        .unwrap_or(range.strip_prefix(['-', '+'])?)
        .parse()
        .ok()
}

fn find_edit_line_numbers(hunks: &[PatchHunk], old_text: &str) -> Option<EditLineNumbers> {
    for hunk in hunks {
        let Some((line_index, old_line)) = find_hunk_line(hunk, old_text, false) else {
            continue;
        };
        let new_line = hunk_new_line_for_old_position(hunk, line_index)?;
        return Some(EditLineNumbers { old_line, new_line });
    }
    None
}

fn hunk_new_line_for_old_position(hunk: &PatchHunk, line_index: usize) -> Option<usize> {
    hunk.lines[line_index]
        .new_line
        .or_else(|| {
            hunk.lines[line_index..]
                .iter()
                .find_map(|line| line.new_line)
        })
        .or_else(|| {
            hunk.lines[..line_index]
                .iter()
                .rev()
                .find_map(|line| line.new_line.map(|line_number| line_number + 1))
        })
}

fn find_hunk_line(hunk: &PatchHunk, needle: &str, new_side: bool) -> Option<(usize, usize)> {
    let needle = needle.replace("\r\n", "\n").replace('\r', "\n");
    let needle = needle.trim_end_matches('\n');
    if needle.is_empty() {
        return None;
    }

    let mut text = String::new();
    let mut starts = Vec::new();
    for (index, line) in hunk.lines.iter().enumerate() {
        let line_number = if new_side {
            line.new_line
        } else {
            line.old_line
        };
        let Some(line_number) = line_number else {
            continue;
        };
        starts.push((text.len(), index, line_number));
        text.push_str(&line.text);
        text.push('\n');
    }
    let offset = text.find(needle)?;
    starts
        .into_iter()
        .take_while(|(start, _, _)| *start <= offset)
        .last()
        .map(|(_, index, line_number)| (index, line_number))
}

pub(crate) fn tool_kind(name: &str) -> acp::ToolKind {
    let name = name.to_ascii_lowercase();
    // Exact Pi builtin names first (avoid substring false-positives).
    match name.as_str() {
        "read" => return acp::ToolKind::Read,
        "bash" => return acp::ToolKind::Execute,
        "edit" | "write" => return acp::ToolKind::Edit,
        "grep" | "find" => return acp::ToolKind::Search,
        // ListDir is detected in the pager via `raw_input.target_directory`
        // (there is no ACP ListDir kind). Keep Other so that branch can match.
        "ls" => return acp::ToolKind::Other,
        _ => {}
    }
    if name.contains("read") {
        acp::ToolKind::Read
    } else if name.contains("write") || name.contains("edit") || name.contains("patch") {
        acp::ToolKind::Edit
    } else if name.contains("delete") || name.contains("remove") {
        acp::ToolKind::Delete
    } else if name.contains("move") || name.contains("rename") {
        acp::ToolKind::Move
    } else if name.contains("search") || name.contains("grep") || name.contains("find") {
        acp::ToolKind::Search
    } else if name.contains("bash") || name.contains("shell") || name.contains("exec") {
        acp::ToolKind::Execute
    } else if name.contains("fetch") || name.contains("web") {
        acp::ToolKind::Fetch
    } else {
        acp::ToolKind::Other
    }
}

fn is_find_tool(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name == "find" || name == "glob" || name.ends_with("_find") || name.contains("glob_file")
}

fn is_ls_tool(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name == "ls" || name == "list_dir" || name == "listdir" || name == "list_directory"
}

/// Rewrite Pi tool args into shapes the native Grok cards already understand.
///
/// - `write` → `variant: "Write"` so Edit cards show "Creating "
/// - `ls` → `target_directory` (ListDir card key)
/// - `grep` → `-i` alias for `ignoreCase`
/// - `find` → `output_mode: files_with_matches`
pub(crate) fn normalize_tool_raw_input(name: &str, args: Option<Value>) -> Option<Value> {
    let mut args = args?;
    let Some(obj) = args.as_object_mut() else {
        return Some(args);
    };
    let lower = name.to_ascii_lowercase();

    if lower == "write" || lower.ends_with("_write") {
        obj.entry("variant".to_string())
            .or_insert_with(|| json!("Write"));
    }

    if is_ls_tool(name) {
        if let Some(path) = obj.get("path").cloned() {
            obj.entry("target_directory".to_string()).or_insert(path);
        } else {
            obj.entry("target_directory".to_string())
                .or_insert_with(|| json!("."));
        }
    }

    if is_find_tool(name) {
        obj.entry("output_mode".to_string())
            .or_insert_with(|| json!("files_with_matches"));
        // Prefer `pattern` as the search term; copy glob-like patterns into
        // `glob_pattern` for extractors that look for it.
        if let Some(pattern) = obj.get("pattern").cloned() {
            obj.entry("glob_pattern".to_string()).or_insert(pattern);
        }
    }

    if lower == "grep" || lower.contains("grep") {
        if let Some(ignore_case) = obj.get("ignoreCase").cloned() {
            obj.entry("-i".to_string()).or_insert(ignore_case);
        }
    }

    Some(args)
}

/// Project Pi tool results into the typed `raw_output` shapes native Grok cards
/// deserialize (`ToolOutput::ReadFile` / `Bash` / `GrepSearch` / `ListDir`).
///
/// Without this conversion the Read card has no path/line metadata, the
/// Execute card/viewer has command only, and Search/ListDir cards show empty
/// structured results — Pi's payload is text `content`, not Grok's tagged
/// tool output enum.
pub(crate) fn normalize_tool_raw_output(
    name: &str,
    args: Option<&Value>,
    result: &Value,
    is_error: bool,
) -> Value {
    if is_ls_tool(name) {
        return ls_tool_output(args, result, is_error);
    }
    match tool_kind(name) {
        acp::ToolKind::Read => read_tool_output(args, result, is_error),
        acp::ToolKind::Execute => {
            let command = args
                .and_then(|value| string(value, &["command", "cmd"]))
                .unwrap_or_default()
                .to_string();
            bash_tool_output(&command, result, is_error)
        }
        acp::ToolKind::Search => {
            if is_find_tool(name) {
                find_tool_output(result, is_error)
            } else {
                grep_tool_output(result, is_error)
            }
        }
        _ => result.clone(),
    }
}

pub(crate) fn pi_result_text(value: &Value) -> String {
    if let Some(text) = value.get("output").and_then(Value::as_str) {
        return text.to_string();
    }
    let source = value.get("content").unwrap_or(value);
    match source {
        Value::Array(items) => items
            .iter()
            .filter_map(|item| {
                string(item, &["text", "content", "message", "output"])
                    .map(str::to_owned)
                    .or_else(|| item.as_str().map(str::to_owned))
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Value::String(text) => text.clone(),
        _ => {
            let text = json_text(source);
            if text == "null" { String::new() } else { text }
        }
    }
}

fn read_tool_output(args: Option<&Value>, result: &Value, is_error: bool) -> Value {
    let text = pi_result_text(result);
    if is_error {
        let message = if text.trim().is_empty() {
            "Read failed".to_string()
        } else {
            text
        };
        return json!({
            "type": "ReadFile",
            "FileReadError": message,
        });
    }

    let path = args
        .and_then(|value| string(value, &["path", "filePath", "file_path", "target_file"]))
        .unwrap_or_default()
        .to_string();
    let offset = args
        .and_then(|value| value.get("offset"))
        .and_then(Value::as_u64)
        .map(|n| n as usize);
    let limit = args
        .and_then(|value| value.get("limit"))
        .and_then(Value::as_u64)
        .map(|n| n as usize);

    // Strip Pi continuation footers for line counting; keep full text for content.
    let body = text
        .split("\n\n[")
        .next()
        .unwrap_or(text.as_str())
        .trim_end_matches('\n');
    let content_lines = if body.is_empty() {
        0
    } else {
        body.lines().count()
    };
    let total_from_footer = text.rsplit_once(" of ").and_then(|(_, rest)| {
        rest.split(|c: char| !c.is_ascii_digit())
            .find(|part| !part.is_empty())
            .and_then(|digits| digits.parse::<usize>().ok())
    });
    let start_index = offset.unwrap_or(1).saturating_sub(1);
    let total_lines = total_from_footer
        .unwrap_or(start_index.saturating_add(content_lines))
        .max(content_lines);

    // Pager Read cards treat FileContent.offset as a 0-based skip count
    // (`start = offset + 1`). Pi's offset is 1-indexed. When Pi omits a window,
    // still publish a 0-based full-file range so the header can show line counts.
    let (stored_offset, stored_limit) = match (offset, limit) {
        (None, None) if content_lines > 0 => (Some(0usize), Some(content_lines)),
        (offset, limit) => (offset.map(|value| value.saturating_sub(1)), limit),
    };

    json!({
        "type": "ReadFile",
        "FileContent": {
            "content": text,
            "absolute_path": path,
            "offset": stored_offset,
            "limit": stored_limit,
            "raw_output": body,
            "total_lines": total_lines,
        }
    })
}

/// Project Pi `grep` text (`path:line: content`) into `ToolOutput::GrepSearch`.
fn grep_tool_output(result: &Value, is_error: bool) -> Value {
    let text = pi_result_text(result);
    let trimmed = text.trim();
    if is_error {
        return json!({
            "type": "GrepSearch",
            "stdout": text.as_bytes().to_vec(),
            "stderr": text.as_bytes().to_vec(),
            "exit_code": 2,
            "match_count": 0,
            "file_matches": [],
        });
    }
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("No matches found") {
        return json!({
            "type": "GrepSearch",
            "stdout": text.as_bytes().to_vec(),
            "stderr": Vec::<u8>::new(),
            "exit_code": 1,
            "match_count": 0,
            "file_matches": [],
        });
    }

    let (file_matches, match_count) = parse_pi_grep_matches(&text);
    json!({
        "type": "GrepSearch",
        "stdout": text.as_bytes().to_vec(),
        "stderr": Vec::<u8>::new(),
        "exit_code": 0,
        "match_count": match_count,
        "file_matches": file_matches,
    })
}

/// Project Pi `find` path list into `ToolOutput::GrepSearch` (files_with_matches).
fn find_tool_output(result: &Value, is_error: bool) -> Value {
    let text = pi_result_text(result);
    let trimmed = text.trim();
    if is_error {
        return json!({
            "type": "GrepSearch",
            "stdout": text.as_bytes().to_vec(),
            "stderr": text.as_bytes().to_vec(),
            "exit_code": 2,
            "match_count": 0,
            "file_matches": [],
        });
    }
    if trimmed.is_empty()
        || trimmed.eq_ignore_ascii_case("No files found matching pattern")
        || trimmed.eq_ignore_ascii_case("No files found")
    {
        return json!({
            "type": "GrepSearch",
            "stdout": text.as_bytes().to_vec(),
            "stderr": Vec::<u8>::new(),
            "exit_code": 0,
            "match_count": 0,
            "file_matches": [],
        });
    }

    // One path per line. Store as stdout so the pager can also recover
    // file_paths via parse_file_paths_from_stdout when file_matches is empty.
    let paths: Vec<&str> = trimmed
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    let match_count = paths.len();
    json!({
        "type": "GrepSearch",
        "stdout": text.as_bytes().to_vec(),
        "stderr": Vec::<u8>::new(),
        "exit_code": 0,
        "match_count": match_count,
        "file_matches": [],
    })
}

/// Project Pi `ls` listing into `ToolOutput::ListDir`.
fn ls_tool_output(args: Option<&Value>, result: &Value, is_error: bool) -> Value {
    let text = pi_result_text(result);
    let path = args
        .and_then(|value| string(value, &["path", "target_directory", "targetDirectory"]))
        .unwrap_or(".")
        .to_string();

    if is_error {
        let message = if text.trim().is_empty() {
            "List directory failed".to_string()
        } else {
            text
        };
        // Prefer NotFound when Pi's message looks like a missing path.
        if message.to_ascii_lowercase().contains("not found")
            || message.to_ascii_lowercase().contains("no such file")
        {
            return json!({ "type": "ListDir", "NotFound": message });
        }
        if message.to_ascii_lowercase().contains("not a directory") {
            return json!({ "type": "ListDir", "NotADirectory": message });
        }
        if message.to_ascii_lowercase().contains("permission") {
            return json!({ "type": "ListDir", "PermissionDenied": message });
        }
        return json!({ "type": "ListDir", "Error": message });
    }

    json!({
        "type": "ListDir",
        "Content": {
            "content": text,
            "absolute_root_path": path,
        }
    })
}

/// Parse Pi grep output lines:
/// - direct match: `path:line: content`
/// - RTK compact match: `line: content` under `> path (N matches):`
/// - context: `path-line- content` (ignored for match_count)
fn parse_pi_grep_matches(text: &str) -> (Vec<Value>, usize) {
    let mut order: Vec<String> = Vec::new();
    let mut by_path: IndexMap<String, Vec<Value>> = IndexMap::new();
    let mut match_count = 0usize;
    let mut current_path: Option<String> = None;

    for raw_line in text.lines() {
        let line = raw_line.trim_end();
        if line.is_empty() {
            continue;
        }
        if let Some(path) = parse_rtk_grep_header(line) {
            current_path = Some(path.to_string());
            continue;
        }
        let parsed = split_pi_grep_match_line(line).or_else(|| {
            current_path.as_deref().and_then(|path| {
                split_rtk_grep_match_line(line)
                    .map(|(line_number, content)| (path, line_number, content))
            })
        });
        if let Some((path, line_number, content)) = parsed {
            match_count += 1;
            if !by_path.contains_key(path) {
                order.push(path.to_string());
            }
            by_path.entry(path.to_string()).or_default().push(json!({
                "line_number": line_number,
                "content": content,
            }));
        }
    }

    let file_matches = order
        .into_iter()
        .filter_map(|path| {
            let matches = by_path.swap_remove(&path)?;
            Some(json!({ "path": path, "matches": matches }))
        })
        .collect();
    (file_matches, match_count)
}

fn parse_rtk_grep_header(line: &str) -> Option<&str> {
    let header = line.strip_prefix("> ")?.strip_suffix(':')?;
    let (path, count) = header.rsplit_once(" (")?;
    count
        .strip_suffix(" matches)")
        .or_else(|| count.strip_suffix(" match)"))
        .and_then(|value| value.parse::<usize>().ok())
        .map(|_| path)
}

fn split_rtk_grep_match_line(line: &str) -> Option<(usize, &str)> {
    let line = line.trim_start();
    let (line_number, content) = line.split_once(':')?;
    let line_number = line_number.parse().ok()?;
    Some((line_number, content.trim_start()))
}

fn split_pi_grep_match_line(line: &str) -> Option<(&str, usize, &str)> {
    // Format: `relative/path:12: content` — path may contain colons on Windows
    // (`C:\...`), so scan from the right for `:digits:`.
    let bytes = line.as_bytes();
    let mut i = bytes.len();
    // Find last ": <content>" separator after a line number.
    while i > 0 {
        // find `:` that starts content
        if let Some(colon_content) = line[..i].rfind(':') {
            let after = &line[colon_content + 1..];
            // content may start with space
            let before = &line[..colon_content];
            if let Some(colon_line) = before.rfind(':') {
                let line_str = &before[colon_line + 1..];
                if let Ok(line_number) = line_str.parse::<usize>() {
                    let path = &before[..colon_line];
                    if !path.is_empty() && line_number > 0 {
                        let content = after.strip_prefix(' ').unwrap_or(after);
                        return Some((path, line_number, content));
                    }
                }
            }
            i = colon_content;
        } else {
            break;
        }
    }
    None
}

pub(crate) fn bash_tool_output(command: &str, result: &Value, is_error: bool) -> Value {
    let text = if result.get("output").and_then(Value::as_str).is_some()
        && result.get("content").is_none()
    {
        // Direct Pi `bash` RPC response: { output, exitCode, ... }.
        result
            .get("output")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    } else {
        pi_result_text(result)
    };

    let exit_code = result
        .get("exitCode")
        .and_then(Value::as_i64)
        .or_else(|| {
            result
                .get("details")
                .and_then(|details| details.get("exitCode"))
                .and_then(Value::as_i64)
        })
        .unwrap_or(if is_error { 1 } else { 0 });

    let truncated = result
        .get("truncated")
        .and_then(Value::as_bool)
        .or_else(|| {
            result
                .pointer("/details/truncation/truncated")
                .and_then(Value::as_bool)
        })
        .unwrap_or(false);

    let output_file = result
        .get("fullOutputPath")
        .and_then(Value::as_str)
        .or_else(|| {
            result
                .pointer("/details/fullOutputPath")
                .and_then(Value::as_str)
        })
        .unwrap_or("")
        .to_string();

    let bytes = text.as_bytes().to_vec();
    let total_bytes = bytes.len();
    json!({
        "type": "Bash",
        "output": bytes,
        "output_for_prompt": text,
        "exit_code": exit_code,
        "command": command,
        "truncated": truncated,
        "signal": null,
        "timed_out": false,
        "description": null,
        "current_dir": "",
        "output_file": output_file,
        "total_bytes": total_bytes,
        "was_bare_echo": false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pi_read_maps_to_native_read_card() {
        assert_eq!(tool_kind("read"), acp::ToolKind::Read);
        assert_eq!(tool_kind("use_skill"), acp::ToolKind::Other);
    }

    #[test]
    fn pi_read_result_projects_native_readfile_raw_output() {
        let raw = normalize_tool_raw_output(
            "read",
            Some(&json!({ "path": "src/lib.rs", "offset": 10, "limit": 20 })),
            &json!({
                "content": [{ "type": "text", "text": "fn main() {}\n// end\n\n[Showing lines 10-11 of 42. Use offset=12 to continue.]" }],
            }),
            false,
        );
        assert_eq!(raw.get("type").and_then(Value::as_str), Some("ReadFile"));
        let file = raw.get("FileContent").expect("FileContent variant");
        assert_eq!(
            file.get("absolute_path").and_then(Value::as_str),
            Some("src/lib.rs")
        );
        assert_eq!(file.get("offset").and_then(Value::as_u64), Some(9));
        assert_eq!(file.get("limit").and_then(Value::as_u64), Some(20));
        assert_eq!(file.get("total_lines").and_then(Value::as_u64), Some(42));
        assert!(
            file.get("raw_output")
                .and_then(Value::as_str)
                .is_some_and(|text| text.contains("fn main()"))
        );
    }

    #[test]
    fn pi_bash_result_projects_native_bash_raw_output() {
        let raw = normalize_tool_raw_output(
            "bash",
            Some(&json!({ "command": "ls -la" })),
            &json!({
                "content": [{ "type": "text", "text": "total 48\nREADME.md\n" }],
                "details": { "fullOutputPath": null },
            }),
            false,
        );
        assert_eq!(raw.get("type").and_then(Value::as_str), Some("Bash"));
        assert_eq!(raw.get("command").and_then(Value::as_str), Some("ls -la"));
        assert_eq!(raw.get("exit_code").and_then(Value::as_i64), Some(0));
        let output = raw
            .get("output_for_prompt")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(output.contains("README.md"));

        let direct = bash_tool_output(
            "echo hi",
            &json!({
                "output": "hi\n",
                "exitCode": 0,
                "truncated": false,
            }),
            false,
        );
        assert_eq!(direct.get("type").and_then(Value::as_str), Some("Bash"));
        assert_eq!(
            direct.get("output_for_prompt").and_then(Value::as_str),
            Some("hi\n")
        );
    }

    #[test]
    fn pi_edit_and_write_inputs_produce_native_diff_content() {
        let edit = edit_diff_content(
            "edit",
            Some(&json!({
                "path": "README.md",
                "oldText": "before\n",
                "newText": "after\n",
            })),
            None,
        )
        .expect("edit input must become a diff");
        let acp::ToolCallContent::Diff(diff) = &edit[0] else {
            panic!("edit input must produce ACP Diff content");
        };
        assert_eq!(diff.path.to_string_lossy(), "README.md");
        assert_eq!(diff.old_text.as_deref(), Some("before\n"));
        assert_eq!(diff.new_text, "after\n");

        let current_edit = edit_diff_content(
            "edit",
            Some(&json!({
                "path": "README.md",
                "edits": [
                    { "oldText": "before\n", "newText": "after\n" },
                    { "oldText": "first\n", "newText": "second\n" },
                ],
            })),
            None,
        )
        .expect("current edit input must become diffs");
        assert_eq!(current_edit.len(), 2);

        let write = edit_diff_content(
            "write",
            Some(&json!({ "path": "README.md", "content": "new file\n" })),
            None,
        )
        .expect("write input must become a diff");
        let acp::ToolCallContent::Diff(diff) = &write[0] else {
            panic!("write input must produce ACP Diff content");
        };
        assert_eq!(diff.old_text, None);
        assert_eq!(diff.new_text, "new file\n");
    }

    #[test]
    fn pi_multiregion_edit_projects_patch_line_numbers() {
        let content = edit_diff_content(
            "edit",
            Some(&json!({
                "path": "src/example.rs",
                "edits": [
                    { "oldText": "before alpha", "newText": "after alpha" },
                    { "oldText": "before beta", "newText": "after beta" },
                ],
            })),
            Some(&json!({
                "details": {
                    "patch": "--- src/example.rs\n+++ src/example.rs\n@@ -7,2 +7,2 @@\n after alpha\n-before alpha\n+after alpha\n@@ -31,1 +31,1 @@\n-before beta\n+after beta\n"
                },
            })),
        )
        .expect("edit input must become diffs");

        let lines = content
            .iter()
            .map(|content| {
                let acp::ToolCallContent::Diff(diff) = content else {
                    panic!("edit input must produce ACP Diff content");
                };
                diff.meta
                    .as_ref()
                    .and_then(|meta| meta.get("new_line"))
                    .and_then(Value::as_u64)
            })
            .collect::<Vec<_>>();
        assert_eq!(lines, vec![Some(8), Some(31)]);
    }

    #[test]
    fn pi_history_edit_projects_direct_patch_line_number() {
        let content = edit_diff_content(
            "edit",
            Some(&json!({
                "path": "src/example.rs",
                "oldText": "before alpha",
                "newText": "after alpha",
            })),
            Some(&json!({
                "patch": "--- src/example.rs\n+++ src/example.rs\n@@ -42,1 +42,1 @@\n-before alpha\n+after alpha\n"
            })),
        )
        .expect("edit input must become a diff");
        let acp::ToolCallContent::Diff(diff) = &content[0] else {
            panic!("edit input must produce ACP Diff content");
        };
        assert_eq!(
            diff.meta
                .as_ref()
                .and_then(|meta| meta.get("new_line"))
                .and_then(Value::as_u64),
            Some(42)
        );
    }

    #[test]
    fn pi_edit_without_matching_patch_keeps_line_metadata_empty() {
        let content = edit_diff_content(
            "edit",
            Some(&json!({
                "path": "src/example.rs",
                "oldText": "before alpha",
                "newText": "after alpha",
            })),
            Some(&json!({
                "details": {
                    "patch": "--- src/example.rs\n+++ src/example.rs\n@@ -7,1 +7,1 @@\n-before beta\n+after beta\n"
                },
            })),
        )
        .expect("edit input must become a diff");
        let acp::ToolCallContent::Diff(diff) = &content[0] else {
            panic!("edit input must produce ACP Diff content");
        };
        assert!(diff.meta.is_none());
    }

    #[test]
    fn pi_builtin_tool_kinds() {
        assert_eq!(tool_kind("read"), acp::ToolKind::Read);
        assert_eq!(tool_kind("bash"), acp::ToolKind::Execute);
        assert_eq!(tool_kind("edit"), acp::ToolKind::Edit);
        assert_eq!(tool_kind("write"), acp::ToolKind::Edit);
        assert_eq!(tool_kind("grep"), acp::ToolKind::Search);
        assert_eq!(tool_kind("find"), acp::ToolKind::Search);
        assert_eq!(tool_kind("ls"), acp::ToolKind::Other);
    }

    #[test]
    fn pi_write_raw_input_gets_write_variant() {
        let args =
            normalize_tool_raw_input("write", Some(json!({ "path": "a.rs", "content": "x" })))
                .unwrap();
        assert_eq!(args.get("variant").and_then(Value::as_str), Some("Write"));
    }

    #[test]
    fn pi_ls_raw_input_gets_target_directory() {
        let args = normalize_tool_raw_input("ls", Some(json!({ "path": "src" }))).unwrap();
        assert_eq!(
            args.get("target_directory").and_then(Value::as_str),
            Some("src")
        );
    }

    #[test]
    fn pi_grep_result_projects_native_grepsearch() {
        let raw = normalize_tool_raw_output(
            "grep",
            Some(&json!({ "pattern": "fn main", "path": "." })),
            &json!({
                "content": [{
                    "type": "text",
                    "text": "src/main.rs:10: fn main() {\nsrc/lib.rs:3: fn main_helper() {\n"
                }],
            }),
            false,
        );
        assert_eq!(raw.get("type").and_then(Value::as_str), Some("GrepSearch"));
        assert_eq!(raw.get("match_count").and_then(Value::as_u64), Some(2));
        let files = raw.get("file_matches").and_then(Value::as_array).unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(
            files[0].get("path").and_then(Value::as_str),
            Some("src/main.rs")
        );
        assert_eq!(
            files[0].get("matches").and_then(Value::as_array).unwrap()[0]
                .get("line_number")
                .and_then(Value::as_u64),
            Some(10)
        );
    }

    #[test]
    fn pi_find_result_projects_files_with_matches() {
        let raw = normalize_tool_raw_output(
            "find",
            Some(&json!({ "pattern": "*.rs" })),
            &json!({
                "content": [{ "type": "text", "text": "src/a.rs\nsrc/b.rs\n" }],
            }),
            false,
        );
        assert_eq!(raw.get("type").and_then(Value::as_str), Some("GrepSearch"));
        assert_eq!(raw.get("match_count").and_then(Value::as_u64), Some(2));
    }

    #[test]
    fn pi_ls_result_projects_native_listdir() {
        let raw = normalize_tool_raw_output(
            "ls",
            Some(&json!({ "path": "src" })),
            &json!({
                "content": [{ "type": "text", "text": "main.rs\nlib.rs\n" }],
            }),
            false,
        );
        assert_eq!(raw.get("type").and_then(Value::as_str), Some("ListDir"));
        let content = raw.get("Content").expect("ListDir Content");
        assert_eq!(
            content.get("absolute_root_path").and_then(Value::as_str),
            Some("src")
        );
        assert!(
            content
                .get("content")
                .and_then(Value::as_str)
                .is_some_and(|t| t.contains("main.rs"))
        );
    }

    #[test]
    fn pi_grep_match_line_parser() {
        let (path, line, content) =
            split_pi_grep_match_line("crates/foo/bar.rs:42: let x = 1;").unwrap();
        assert_eq!(path, "crates/foo/bar.rs");
        assert_eq!(line, 42);
        assert_eq!(content, "let x = 1;");
        assert!(split_pi_grep_match_line("crates/foo/bar.rs-42- context").is_none());
    }

    #[test]
    fn pi_rtk_grep_output_projects_matches_under_file_header() {
        let raw = normalize_tool_raw_output(
            "grep",
            Some(&json!({ "pattern": "pi-grok-adapter", "path": "Cargo.toml" })),
            &json!({
                "content": [{
                    "type": "text",
                    "text": "1 matches in 1 files:\n\n> Cargo.toml (1 matches):\n    6: \"crates/codegen/pi-grok-adapter\",\n"
                }]
            }),
            false,
        );
        assert_eq!(raw.get("match_count").and_then(Value::as_u64), Some(1));
        let file = &raw.get("file_matches").and_then(Value::as_array).unwrap()[0];
        assert_eq!(file.get("path").and_then(Value::as_str), Some("Cargo.toml"));
        assert_eq!(
            file.get("matches").and_then(Value::as_array).unwrap()[0]
                .get("line_number")
                .and_then(Value::as_u64),
            Some(6)
        );
    }
}
