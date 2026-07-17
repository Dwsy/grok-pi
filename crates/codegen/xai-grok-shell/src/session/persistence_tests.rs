use super::*;

struct ActorGuard {
    handle: PersistenceHandle,
    task: tokio::task::JoinHandle<()>,
}

impl ActorGuard {
    async fn stop(self) {
        self.task.abort();
        let _ = self.task.await;
    }
}

fn test_actor(info: Info, storage: Arc<dyn StorageAdapter>) -> ActorGuard {
    let (tx, rx) = mpsc::unbounded_channel();
    let summary_tx = tx.clone();
    let sampling_client = OaiCompatClient::new(xai_grok_sampler::SamplerConfig::default()).unwrap();
    let task = tokio::spawn(
        SessionPersistence {
            info,
            storage,
            pending_notification: None,
            rx,
            remote_sync: None,
            relay_sync: None,
            summary: crate::session::summary::SummaryGenerator::new(
                crate::session::summary::SummaryConfig {
                    sampling_client,
                    model: String::new(),
                    persistence_tx: summary_tx,
                },
            ),
            registry_title_sync: None,
            gateway: None,
        }
        .run(),
    );
    ActorGuard {
        handle: PersistenceHandle { tx, noop: false },
        task,
    }
}

fn notification(info: &Info, text: &str) -> acp::SessionNotification {
    acp::SessionNotification::new(
        info.id.clone(),
        acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(acp::ContentBlock::Text(
            acp::TextContent::new(text),
        ))),
    )
}

fn neutral_update(info: &Info, text: &str) -> SessionUpdate {
    SessionUpdate::Acp(Box::new(notification(info, text)))
}

#[test]
fn committed_error_does_not_restore_pending_notification() {
    let notification = notification(
        &Info {
            id: acp::SessionId::new("committed-update"),
            cwd: "/test".into(),
        },
        "committed",
    );
    let mut pending = None;
    let result = SessionPersistence::finish_pending_append(
        &mut pending,
        notification,
        Err(crate::session::storage::AppendUpdateError::Committed(
            io::Error::other("summary patch failed"),
        )),
    );
    assert_eq!(result.unwrap_err().to_string(), "summary patch failed");
    assert!(pending.is_none());
}

#[test]
fn uncommitted_error_restores_pending_notification() {
    let notification = notification(
        &Info {
            id: acp::SessionId::new("uncommitted-update"),
            cwd: "/test".into(),
        },
        "pending",
    );
    let mut pending = None;
    let result = SessionPersistence::finish_pending_append(
        &mut pending,
        notification,
        Err(crate::session::storage::AppendUpdateError::NotCommitted(
            io::Error::other("append failed"),
        )),
    );
    assert!(result.is_err());
    assert!(pending.is_some());
}

#[tokio::test]
async fn durable_ack_drains_pending_update_in_fifo_order() {
    let dir = tempfile::tempdir().unwrap();
    let info = Info {
        id: acp::SessionId::new("durable-update"),
        cwd: dir.path().to_string_lossy().into_owned(),
    };
    let storage = Arc::new(JsonlStorageAdapter::with_explicit_session_dir(
        dir.path().to_path_buf(),
    ));
    storage
        .init_session(&info, default_model_id())
        .await
        .unwrap();
    let actor = test_actor(info.clone(), storage.clone());
    actor
        .handle
        .tx
        .send(PersistenceMsg::Update(neutral_update(&info, "before")))
        .unwrap();
    let (respond_to, response) = tokio::sync::oneshot::channel();
    actor
        .handle
        .tx
        .send(PersistenceMsg::AppendUpdateDurablyAndAck {
            update: neutral_update(&info, "durable"),
            respond_to,
        })
        .unwrap();
    response.await.unwrap().unwrap();
    let summary = storage.load_summary(&info).await.unwrap();
    assert_eq!(summary.num_messages, 2);

    let updates = storage.load_session(&info).await.unwrap().updates;
    let texts = updates
        .iter()
        .filter_map(|update| {
            let SessionUpdate::Acp(notification) = update else {
                return None;
            };
            let acp::SessionUpdate::AgentMessageChunk(chunk) = &notification.update else {
                return None;
            };
            let acp::ContentBlock::Text(text) = &chunk.content else {
                return None;
            };
            Some(text.text.clone())
        })
        .collect::<Vec<_>>();
    assert_eq!(texts, ["before", "durable"]);
    actor.stop().await;
}
