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
                d.input_cost + d.output_cost + d.cache_read_cost + d.cache_write_cost
           FROM sessions s LEFT JOIN session_details_cache d ON d.path = s.path
          ORDER BY s.modified DESC"
    } else {
        "SELECT s.id, s.path, s.cwd, s.name, s.created, s.modified, s.message_count,
                COALESCE(s.first_message, ''), COALESCE(d.models_json, '[]'),
                COALESCE(d.input_tokens, 0), COALESCE(d.output_tokens, 0),
                COALESCE(d.cache_read_tokens, 0), COALESCE(d.cache_write_tokens, 0),
                d.input_cost + d.output_cost + d.cache_read_cost + d.cache_write_cost
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
    })
}
