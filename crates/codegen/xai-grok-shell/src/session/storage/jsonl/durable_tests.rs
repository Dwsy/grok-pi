use super::*;
use crate::session::info::Info;
use crate::session::persistence::default_model_id;
use crate::session::storage::{SessionUpdate, StorageAdapter};

fn info() -> Info {
    Info {
        id: acp::SessionId::new("durable-jsonl"),
        cwd: "/test".into(),
    }
}

fn update(info: &Info, text: String) -> SessionUpdate {
    SessionUpdate::Acp(Box::new(acp::SessionNotification::new(
        info.id.clone(),
        acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(acp::ContentBlock::Text(
            acp::TextContent::new(text),
        ))),
    )))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ordinary_and_durable_appends_keep_every_physical_line_parseable() {
    const N: usize = 100;
    let dir = tempfile::tempdir().unwrap();
    let info = info();
    let adapter = JsonlStorageAdapter::with_explicit_session_dir(dir.path().to_path_buf());
    adapter
        .init_session(&info, default_model_id())
        .await
        .unwrap();
    let ordinary = adapter.clone();
    let durable = adapter.clone();
    let info_a = info.clone();
    let info_b = info.clone();
    let ordinary = tokio::spawn(async move {
        for index in 0..N {
            ordinary
                .append_update(&info_a, &update(&info_a, format!("ordinary-{index}")))
                .await
                .unwrap();
        }
    });
    let durable = tokio::spawn(async move {
        for index in 0..N {
            durable
                .append_update_durable(&info_b, &update(&info_b, format!("durable-{index}")))
                .await
                .unwrap();
        }
    });
    ordinary.await.unwrap();
    durable.await.unwrap();

    let bytes = std::fs::read(dir.path().join("updates.jsonl")).unwrap();
    let parsed = bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(serde_json::from_slice::<SessionUpdateEnvelope>)
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(parsed.len(), N * 2);
}

#[tokio::test]
async fn append_commit_is_reported_when_bookkeeping_fails() {
    let dir = tempfile::tempdir().unwrap();
    let info = info();
    let adapter = JsonlStorageAdapter::with_explicit_session_dir(dir.path().to_path_buf());
    adapter
        .init_session(&info, default_model_id())
        .await
        .unwrap();
    let result = adapter
        .append_update_with_bookkeeping(&info, &update(&info, "committed".into()), async {
            Err(io::Error::other("summary patch failed"))
        })
        .await;
    assert!(matches!(
        result,
        Err(crate::session::storage::AppendUpdateError::Committed(_))
    ));
    let bytes = std::fs::read(dir.path().join("updates.jsonl")).unwrap();
    let parsed = bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(serde_json::from_slice::<SessionUpdateEnvelope>)
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(parsed.len(), 1);
}

#[test]
fn lock_serializes_tail_heal_and_complete_record() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("updates.jsonl");
    std::fs::write(&path, b"torn").unwrap();
    JsonlStorageAdapter::append_jsonl_line_sync(
        &path,
        b"{\"record\":1}\n".to_vec(),
        AppendDurability::Buffered,
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(path).unwrap(),
        "torn\n{\"record\":1}\n"
    );
}

#[test]
fn directory_barrier_failure_is_retried_even_after_file_exists() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static ATTEMPTS: AtomicUsize = AtomicUsize::new(0);
    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = TEST_LOCK.lock().unwrap();
    fn sync_file(file: &std::fs::File) -> io::Result<()> {
        file.sync_all()
    }
    fn flaky_parent(_path: &Path) -> io::Result<()> {
        if ATTEMPTS.fetch_add(1, Ordering::SeqCst) == 0 {
            Err(io::Error::other("directory barrier failed"))
        } else {
            Ok(())
        }
    }

    ATTEMPTS.store(0, Ordering::SeqCst);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("updates.jsonl");
    assert!(
        JsonlStorageAdapter::append_jsonl_line_sync_with(
            &path,
            b"{\"record\":1}\n".to_vec(),
            AppendDurability::Durable,
            sync_file,
            flaky_parent,
        )
        .is_err()
    );
    JsonlStorageAdapter::append_jsonl_line_sync_with(
        &path,
        b"{\"record\":1}\n".to_vec(),
        AppendDurability::Durable,
        sync_file,
        flaky_parent,
    )
    .unwrap();
    assert_eq!(ATTEMPTS.load(Ordering::SeqCst), 2);
}

#[test]
fn file_barrier_error_propagates() {
    fn fail(_file: &std::fs::File) -> io::Result<()> {
        Err(io::Error::other("file barrier failed"))
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("updates.jsonl");
    let error = JsonlStorageAdapter::append_jsonl_line_sync_with(
        &path,
        b"{\"record\":1}\n".to_vec(),
        AppendDurability::Durable,
        fail,
        |_| Ok(()),
    )
    .unwrap_err();
    assert_eq!(error.to_string(), "file barrier failed");
}

#[cfg(target_os = "macos")]
#[test]
fn darwin_fullfsync_seam_reports_invalid_descriptor() {
    assert!(JsonlStorageAdapter::fullfsync_raw(-1).is_err());
}
