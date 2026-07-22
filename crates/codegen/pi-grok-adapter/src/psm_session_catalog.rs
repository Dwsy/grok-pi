use crate::model::PiSessionInfo;
use rusqlite::{Connection, OpenFlags, params};
use serde_json::Value;
use std::{
    net::{SocketAddr, TcpStream},
    path::{Path, PathBuf},
    time::Duration,
};

const DEFAULT_PSM_WS_PORT: u16 = 52_131;
const CONNECT_TIMEOUT: Duration = Duration::from_millis(150);
const BUSY_TIMEOUT: Duration = Duration::from_millis(50);

/// PSM is considered available only while its configured local server port is
/// accepting connections. A stale SQLite file is not evidence that PSM runs.
pub fn load_catalog(cwd: &Path, all: bool) -> Option<Vec<PiSessionInfo>> {
    if !psm_server_is_listening(DEFAULT_PSM_WS_PORT) {
        return None;
    }
    load_catalog_from_db(&default_database_path()?, cwd, all).ok()
}

fn default_database_path() -> Option<PathBuf> {
    Some(std::env::var_os("HOME")?.into()).map(|home: PathBuf| {
        home.join(".pi")
            .join("agent")
            .join("sessions")
            .join("sessions.db")
    })
}

fn psm_server_is_listening(port: u16) -> bool {
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    TcpStream::connect_timeout(&address, CONNECT_TIMEOUT).is_ok()
}

fn load_catalog_from_db(
    db_path: &Path,
    cwd: &Path,
    all: bool,
) -> rusqlite::Result<Vec<PiSessionInfo>> {
    let connection = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.busy_timeout(BUSY_TIMEOUT)?;
    connection.execute_batch("PRAGMA query_only = ON;")?;
    let sql = if all {
        "SELECT s.id, s.path, s.cwd, s.name, s.created, s.modified, s.message_count,
                COALESCE(s.first_message, ''), COALESCE(d.models_json, '[]'),
                COALESCE(d.input_tokens, 0), COALESCE(d.output_tokens, 0),
                COALESCE(d.cache_read_tokens, 0), COALESCE(d.cache_write_tokens, 0),
                d.input_cost + d.output_cost + d.cache_read_cost + d.cache_write_cost,
                s.parent_session_path
           FROM sessions s LEFT JOIN session_details_cache d ON d.path = s.path
          ORDER BY s.modified DESC"
    } else {
        "SELECT s.id, s.path, s.cwd, s.name, s.created, s.modified, s.message_count,
                COALESCE(s.first_message, ''), COALESCE(d.models_json, '[]'),
                COALESCE(d.input_tokens, 0), COALESCE(d.output_tokens, 0),
                COALESCE(d.cache_read_tokens, 0), COALESCE(d.cache_write_tokens, 0),
                d.input_cost + d.output_cost + d.cache_read_cost + d.cache_write_cost,
                s.parent_session_path
           FROM sessions s LEFT JOIN session_details_cache d ON d.path = s.path
          WHERE s.cwd = ?1 ORDER BY s.modified DESC"
    };
    let mut statement = connection.prepare(sql)?;
    let rows = if all {
        statement.query_map([], session_from_row)?
    } else {
        statement.query_map(params![cwd.to_string_lossy()], session_from_row)?
    };
    rows.collect()
}

/// A single full-text search hit from PSM's FTS5 index.
#[derive(Debug, Clone)]
pub struct PsmSearchHit {
    pub session_id: String,
    pub cwd: String,
    pub summary: String,
    pub updated_at: String,
    pub score: f32,
    pub matched_fields: Vec<String>,
    pub snippet: Option<String>,
}

/// Full-text search across PSM's FTS5 index (sessions_fts + message_fts).
/// Returns ranked results with snippets. Falls back to `None` if PSM is
/// unavailable or the query is empty.
pub fn full_text_search(
    cwd: Option<&Path>,
    query: &str,
    limit: usize,
) -> Option<Vec<PsmSearchHit>> {
    let query = query.trim();
    if query.is_empty() {
        return Some(Vec::new());
    }
    if !psm_server_is_listening(DEFAULT_PSM_WS_PORT) {
        return None;
    }
    let db_path = default_database_path()?;
    full_text_search_db(&db_path, cwd, query, limit).ok()
}

fn full_text_search_db(
    db_path: &Path,
    cwd: Option<&Path>,
    query: &str,
    limit: usize,
) -> rusqlite::Result<Vec<PsmSearchHit>> {
    let connection = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.busy_timeout(BUSY_TIMEOUT)?;
    connection.execute_batch("PRAGMA query_only = ON;")?;

    // Use nullable cwd parameter: (?2 IS NULL OR s.cwd = ?2)
    let cwd_val: Option<String> = cwd.map(|c| c.to_string_lossy().to_string());

    // Try the strict AND query first; if it finds nothing, fall back to an
    // OR query so multi-word searches still surface partial matches
    // (mirrors codex's resume-picker AND→OR fallback).
    let and_query = build_fts_query(query);
    if and_query.is_empty() {
        return Ok(Vec::new());
    }
    let hits = run_fts_search(&connection, &and_query, cwd_val.as_deref(), limit)?;
    if !hits.is_empty() {
        return Ok(hits);
    }
    let or_query = build_fts_query_or(query);
    if or_query == and_query {
        return Ok(hits);
    }
    run_fts_search(&connection, &or_query, cwd_val.as_deref(), limit)
}

/// Execute one FTS5 MATCH pass across `sessions_fts` (title + content) and,
/// if needed to fill `limit`, `message_fts` (deeper message bodies).
fn run_fts_search(
    connection: &Connection,
    fts_query: &str,
    cwd_val: Option<&str>,
    limit: usize,
) -> rusqlite::Result<Vec<PsmSearchHit>> {
    // Search sessions_fts first (title + content level).
    let mut hits: Vec<PsmSearchHit> = Vec::new();
    let mut seen_ids = std::collections::HashSet::new();

    // Query sessions_fts: matches on name, first_message, user/assistant text.
    let session_sql =
        "SELECT s.id, s.cwd, COALESCE(s.name, s.first_message, ''), s.modified,
                bm25(sessions_fts, 0, 10.0, 5.0, 5.0, 1.0, 1.0) AS rank,
                snippet(sessions_fts, 4, '[', ']', ' … ', 24) AS snip
           FROM sessions_fts
           JOIN sessions s ON s.rowid = sessions_fts.rowid
          WHERE sessions_fts MATCH ?1 AND (?2 IS NULL OR s.cwd = ?2)
          ORDER BY rank ASC, s.modified DESC
          LIMIT ?3";

    let mut stmt = connection.prepare(session_sql)?;
    let rows = stmt.query_map(params![fts_query, cwd_val, limit as i64], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, f64>(4)?,
            row.get::<_, Option<String>>(5)?,
        ))
    })?;

    for row in rows {
        let (id, cwd_str, summary, modified, rank, snip) = row?;
        seen_ids.insert(id.clone());
        let mut matched_fields = Vec::new();
        if let Some(ref s) = snip {
            if s.contains('[') {
                matched_fields.push("content".to_string());
            }
        }
        if matched_fields.is_empty() {
            matched_fields.push("title".to_string());
        }
        hits.push(PsmSearchHit {
            session_id: id,
            cwd: cwd_str,
            summary,
            updated_at: modified,
            score: -(rank as f32),
            matched_fields,
            snippet: snip.filter(|s| !s.is_empty()),
        });
    }

    // Supplement with message_fts for deeper content matches.
    if hits.len() < limit {
        let remaining = limit - hits.len();
        let msg_sql =
            "SELECT s.id, s.cwd, COALESCE(s.name, s.first_message, ''), s.modified,
                    bm25(message_fts, 0, 0, 0, 1.0) AS rank,
                    snippet(message_fts, 3, '[', ']', ' … ', 24) AS snip
               FROM message_fts
               JOIN sessions s ON s.path = message_fts.session_path
              WHERE message_fts MATCH ?1 AND (?2 IS NULL OR s.cwd = ?2)
                AND s.id NOT IN (SELECT value FROM json_each(?4))
              ORDER BY rank ASC, s.modified DESC
              LIMIT ?3";
        let seen_json = serde_json::to_string(&seen_ids.iter().collect::<Vec<_>>())
            .unwrap_or_else(|_| "[]".to_string());
        let mut msg_stmt = connection.prepare(msg_sql)?;
        let msg_rows = msg_stmt.query_map(
            params![fts_query, cwd_val, remaining as i64, seen_json],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, f64>(4)?,
                    row.get::<_, Option<String>>(5)?,
                ))
            },
        )?;
        for row in msg_rows {
            let (id, cwd_str, summary, modified, rank, snip) = row?;
            hits.push(PsmSearchHit {
                session_id: id,
                cwd: cwd_str,
                summary,
                updated_at: modified,
                score: -(rank as f32),
                matched_fields: vec!["content".to_string()],
                snippet: snip.filter(|s| !s.is_empty()),
            });
        }
    }

    Ok(hits)
}

/// Build a safe FTS5 match expression from user input.
///
/// Each token is quoted and given a `*` prefix-match suffix so partial
/// words match (e.g. `resum` → `"resum"*` matches "resume", "resumed").
/// Tokens are joined with the given operator (`AND` or `OR`).
fn build_fts_query_with(input: &str, op: &str) -> String {
    input
        .split_whitespace()
        .filter(|t| !t.is_empty())
        .map(|token| {
            // Escape double quotes inside the token, then wrap in quotes
            // with a trailing `*` for prefix matching.
            let escaped = token.replace('"', "\"\"");
            format!("\"{escaped}\" *")
        })
        .collect::<Vec<_>>()
        .join(&format!(" {op} "))
}

/// Build the primary (AND-joined) FTS5 match expression.
fn build_fts_query(input: &str) -> String {
    build_fts_query_with(input, "AND")
}

/// Build the fallback (OR-joined) FTS5 match expression.
/// Used when the AND query returns zero results.
fn build_fts_query_or(input: &str) -> String {
    build_fts_query_with(input, "OR")
}

fn session_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<PiSessionInfo> {
    let models: String = row.get(8)?;
    let model_id = serde_json::from_str::<Value>(&models)
        .ok()
        .and_then(|value| value.as_array()?.last()?.as_str().map(str::to_owned));
    let token_total = [
        row.get::<_, u64>(9)?,
        row.get(10)?,
        row.get(11)?,
        row.get(12)?,
    ]
    .into_iter()
    .sum();
    Ok(PiSessionInfo {
        id: row.get(0)?,
        path: PathBuf::from(row.get::<_, String>(1)?),
        cwd: row.get(2)?,
        name: row.get(3)?,
        created_at: row.get(4)?,
        modified_at: row.get(5)?,
        message_count: row.get::<_, i64>(6)?.max(0) as usize,
        first_message: row.get(7)?,
        model_id,
        total_tokens: Some(token_total),
        total_cost: row.get(13)?,
        parent_session_path: row.get(14)?,
    })
}
