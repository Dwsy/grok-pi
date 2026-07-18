use crate::{
    background_bash_bridge::{
        background_bash_notification, background_bash_output_update, parse_background_bash_message,
        parse_background_bash_tool_result,
    },
    context_projection::{
        build_session_info_response, context_tokens_from_stats, context_tokens_from_usage,
        entries_to_messages_value,
    },
    model::{
        PiCommand, PiHistoryItem, PiModel, PiSessionSwitch, PiSessionTree, PiState, PiToolContent,
        extract_delta, json_text, parse_commands, parse_messages, parse_models, parse_session_switch,
        parse_session_tree, parse_state, scan_local_sessions, scan_local_sessions_for_cwd, string,
    },
    prompt_bridge::{
        direct_bash_command, format_bash_result, prompt_response, prompt_streaming_behavior,
        prompt_to_pi, queue_lane_for_behavior,
    },
    pi_rpc::PiRpc,
    queue_bridge::{QueueLane, QueueMirror, queue_changed_params, string_list},
    recap_bridge::{parse_recap_message, session_recap_notification},
    subagent_projection::{BridgeOperation, bridge_parent_session_id, parse_bridge_message},
    todo_bridge::plan_update_for_tool,
    tool_projection::{
        bash_tool_output, edit_diff_content, history_tool_content, normalize_tool_raw_input,
        normalize_tool_raw_output, pi_result_text, tool_content, tool_kind,
    },
};
use agent_client_protocol as acp;
use anyhow::{Result, anyhow, bail};
use indexmap::IndexMap;
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    rc::Rc,
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, oneshot};
use xai_acp_lib::{AcpClientMessage, acp_send};

#[derive(Debug, Clone)]
pub struct PiBootstrap {
    state: PiState,
    models: Vec<PiModel>,
    commands: Vec<PiCommand>,
}

impl PiBootstrap {
    pub async fn load(rpc: &PiRpc) -> Result<Self> {
        let state = parse_state(&rpc.request(json!({ "type": "get_state" })).await?);
        let mut models = parse_models(
            &rpc.request(json!({ "type": "get_available_models" }))
                .await?,
        );
        if let Some(current) = state.model.clone()
            && !models
                .iter()
                .any(|model| model.provider == current.provider && model.id == current.id)
        {
            models.push(current);
        }
        let commands = parse_commands(&rpc.request(json!({ "type": "get_commands" })).await?);
        Ok(Self {
            state,
            models,
            commands,
        })
    }

    pub fn acp_models(&self) -> Option<acp::SessionModelState> {
        let (available, current) = build_model_catalog(
            &self.models,
            self.state.model.as_ref(),
            &self.state.thinking_level,
        );
        let current = current.or_else(|| available.first().map(|(id, _)| id.clone()))?;
        Some(acp::SessionModelState::new(
            current,
            available.into_values().collect(),
        ))
    }

    pub fn acp_commands(&self) -> Vec<acp::AvailableCommand> {
        command_catalog(&self.commands)
    }

    /// Pi session identifier used to seed the native Grok session surface.
    pub fn session_id(&self) -> &str {
        &self.state.session_id
    }

    /// Optional Pi session title used for Grok's terminal title and header.
    pub fn session_title(&self) -> Option<&str> {
        self.state.session_name.as_deref()
    }
}

struct ActivePrompt {
    id: u64,
    /// Pager-minted id (`_meta.promptId`); echoed on PromptResponse so non-running
    /// mid-turn completions are discarded instead of emitting phantom turns.
    client_prompt_id: Option<String>,
    completion: oneshot::Sender<PromptCompletion>,
    agent_started: bool,
    cancelled: bool,
}

struct PromptCompletion {
    reason: acp::StopReason,
    client_prompt_id: Option<String>,
}

#[derive(Debug, Clone, Copy, Default)]
struct StreamSeen {
    text: bool,
    thought: bool,
}

#[derive(Default)]
struct PendingSubagentBridge {
    target_session_id: Option<String>,
    events: Vec<Value>,
}

impl PendingSubagentBridge {
    fn begin(&mut self, target_session_id: &str) -> Result<()> {
        if let Some(existing) = &self.target_session_id {
            bail!("Pi session transition to {existing} is already in progress");
        }
        self.target_session_id = Some(target_session_id.to_string());
        self.events.clear();
        Ok(())
    }

    fn defer_if_targeted(&mut self, event: &Value) -> Result<bool> {
        let Some(parent_session_id) = bridge_parent_session_id(event)? else {
            return Ok(false);
        };
        let Some(target_session_id) = &self.target_session_id else {
            return Ok(false);
        };
        if parent_session_id != target_session_id {
            return Ok(false);
        }
        self.events.push(event.clone());
        Ok(true)
    }

    fn commit_if_target(&mut self, target_session_id: &str) -> Result<Vec<Value>> {
        match self.target_session_id.as_deref() {
            None => Ok(Vec::new()),
            Some(current) if current == target_session_id => {
                self.target_session_id = None;
                Ok(std::mem::take(&mut self.events))
            }
            Some(current) => bail!(
                "Pi session transition to {current} is still pending while {target_session_id} loads"
            ),
        }
    }

    fn abandon(&mut self, target_session_id: &str) {
        if self.target_session_id.as_deref() == Some(target_session_id) {
            self.target_session_id = None;
            self.events.clear();
        }
    }
}

struct AdapterState {
    bootstrap: PiBootstrap,
    acp_session_id: String,
    model_map: HashMap<String, PiModel>,
    active_prompts: Vec<ActivePrompt>,
    next_prompt_id: u64,
    bash_running: bool,
    live_assistant: Option<StreamSeen>,
    session_dir: PathBuf,
    session_paths: HashMap<String, PathBuf>,
    /// Pi tool args keyed by toolCallId. End events may omit args; the pager
    /// still needs path/command when projecting native Read/Execute cards.
    tool_args: HashMap<String, Value>,
    /// Latest Pi context-window usage (tokens used). Stamped on ACP session
    /// updates as `_meta.totalTokens` so Grok's native context bar can render.
    last_context_tokens: Option<u64>,
    /// Local timing only; Pi owns compaction itself and reports its token result.
    compaction_started_at: Option<Instant>,
    /// Pi steering / follow-up queue mirrored as Grok `x.ai/queue/changed`.
    queue_mirror: QueueMirror,
    /// Last accepted live bridge sequence per child. The adapter uses this only
    /// to reject duplicate/out-of-order transport events; child lifecycle stays
    /// owned by the Pi extension.
    subagent_bridge_sequences: HashMap<String, u64>,
    /// Pi emits extension `session_start` events before its `switch_session`
    /// response reaches ACP. Buffer target-session subagent replay until ACP
    /// commits the matching session load, rather than validating it against the
    /// still-active Pager session.
    pending_subagent_bridge: PendingSubagentBridge,
}

#[derive(Clone)]
pub struct PiAgent {
    rpc: PiRpc,
    client_tx: mpsc::UnboundedSender<AcpClientMessage>,
    state: Rc<RefCell<AdapterState>>,
    bash_control_meta: Option<PathBuf>,
}

impl PiAgent {
    pub fn new(
        rpc: PiRpc,
        client_tx: mpsc::UnboundedSender<AcpClientMessage>,
        bootstrap: PiBootstrap,
        session_dir: PathBuf,
        bash_control_meta: Option<PathBuf>,
    ) -> Self {
        let acp_session_id = bootstrap.state.session_id.clone();
        let model_map = bootstrap
            .models
            .iter()
            .cloned()
            .map(|model| (model_key(&model), model))
            .collect();
        Self {
            rpc,
            client_tx,
            bash_control_meta,
            state: Rc::new(RefCell::new(AdapterState {
                bootstrap,
                acp_session_id,
                model_map,
                active_prompts: Vec::new(),
                next_prompt_id: 1,
                bash_running: false,
                live_assistant: None,
                session_dir,
                session_paths: HashMap::new(),
                tool_args: HashMap::new(),
                last_context_tokens: None,
                compaction_started_at: None,
                queue_mirror: QueueMirror::default(),
                subagent_bridge_sequences: HashMap::new(),
                pending_subagent_bridge: PendingSubagentBridge::default(),
            })),
        }
    }

    pub async fn run_events(self: Rc<Self>, mut events: mpsc::UnboundedReceiver<Value>) {
        while let Some(event) = events.recv().await {
            if let Err(error) = self.handle_event(event).await {
                tracing::warn!(%error, "failed to adapt Pi event into Grok ACP");
                self.send_ui_notification(&format!("Pi adapter: {error}"), Some("warning"))
                    .await;
            }
        }
        self.finish_prompts(acp::StopReason::Cancelled);
    }

    pub async fn refresh(&self) -> Result<PiBootstrap> {
        let bootstrap = PiBootstrap::load(&self.rpc).await?;
        self.replace_bootstrap(bootstrap.clone());
        Ok(bootstrap)
    }

    /// Publish Pi's local session catalog for Grok's existing native picker.
    ///
    /// Pi keeps ownership of the JSONL format and of switching; this read-only
    /// metadata projection only gives the pager a selectable catalog.
    pub async fn publish_session_catalog(&self, cwd: PathBuf, all: bool) {
        let session_dir = {
            let state = self.state.borrow();
            catalog_session_dir(&state.bootstrap.state, &state.session_dir)
        };
        let sessions = tokio::task::spawn_blocking(move || {
            if all {
                scan_local_sessions(&session_dir)
            } else {
                scan_local_sessions_for_cwd(&session_dir, &cwd)
            }
        })
        .await
        .unwrap_or_default();
        let paths: HashMap<_, _> = sessions
            .iter()
            .map(|session| (session.id.clone(), session.path.clone()))
            .collect();
        self.state.borrow_mut().session_paths.extend(paths);
        self.send_ext_notification(
            "pi/ui/session_catalog",
            json!({
                "scope": if all { "all" } else { "current" },
                "sessions": sessions.into_iter().map(|session| json!({
                    "id": session.id,
                    "summary": session.name.unwrap_or(session.first_message),
                    "cwd": session.cwd,
                    "createdAt": session.created_at,
                    "updatedAt": session.modified_at,
                    "messageCount": session.message_count,
                })).collect::<Vec<_>>(),
            }),
        )
        .await;
    }

    /// Request Pi to replace its active session. The adapter publishes the new
    /// session identity only after Pi accepts the switch and its replacement
    /// state can be loaded successfully.
    pub async fn switch_session(
        &self,
        session_path: &Path,
        expected_session_id: &str,
    ) -> Result<PiSessionSwitch> {
        self.state
            .borrow_mut()
            .pending_subagent_bridge
            .begin(expected_session_id)?;
        let response = match self
            .rpc
            .request(json!({
                "type": "switch_session",
                "sessionPath": session_path,
            }))
            .await
        {
            Ok(response) => response,
            Err(error) => {
                self.state
                    .borrow_mut()
                    .pending_subagent_bridge
                    .abandon(expected_session_id);
                return Err(error);
            }
        };
        let result = parse_session_switch(&response);
        if result.cancelled {
            self.state
                .borrow_mut()
                .pending_subagent_bridge
                .abandon(expected_session_id);
            return Ok(result);
        }
        let bootstrap = match PiBootstrap::load(&self.rpc).await {
            Ok(bootstrap) => bootstrap,
            Err(error) => {
                self.state
                    .borrow_mut()
                    .pending_subagent_bridge
                    .abandon(expected_session_id);
                return Err(error);
            }
        };
        if bootstrap.state.session_id != expected_session_id {
            self.state
                .borrow_mut()
                .pending_subagent_bridge
                .abandon(expected_session_id);
            bail!(
                "Pi switched to {}, not requested session {expected_session_id}",
                bootstrap.state.session_id
            );
        }
        self.replace_bootstrap(bootstrap);
        Ok(result)
    }

    /// Read-only projection of Pi's current entry tree (`get_tree`).
    ///
    /// Parse + flatten + drop of the nested Value happen on a large-stack
    /// worker: long sessions produce trees deep enough to overflow the default
    /// Tokio worker stack even after serde_json recursion limits are disabled.
    async fn fetch_session_tree(&self) -> Result<PiSessionTree> {
        let data = self.rpc.request(json!({ "type": "get_tree" })).await?;
        let tree = tokio::task::spawn_blocking(move || {
            crate::pi_rpc::with_large_stack(move || {
                let tree = parse_session_tree(&data);
                drop(data);
                tree
            })
        })
        .await
        .map_err(|error| anyhow!("Pi get_tree worker failed: {error}"))?;
        Ok(tree)
    }

    /// Run a hidden bridge extension command (`/__pi_*`) and wait for the
    /// non-agent preflight probe to complete.
    async fn run_bridge_command(&self, command: &str, args: &str) -> Result<(), acp::Error> {
        let message = if args.trim().is_empty() {
            format!("/{command}")
        } else {
            format!("/{command} {args}")
        };
        let (completion_tx, completion_rx) = oneshot::channel();
        let prompt_id = {
            let mut state = self.state.borrow_mut();
            let prompt_id = state.next_prompt_id;
            state.next_prompt_id = state.next_prompt_id.wrapping_add(1).max(1);
            state.active_prompts.push(ActivePrompt {
                id: prompt_id,
                client_prompt_id: None,
                completion: completion_tx,
                agent_started: false,
                cancelled: false,
            });
            prompt_id
        };
        let request = json!({ "type": "prompt", "message": message });
        if let Err(error) = self.rpc.request(request).await {
            self.remove_prompt(prompt_id);
            return Err(acp_internal(error));
        }
        let probe = self.clone();
        tokio::task::spawn_local(async move {
            probe.probe_prompt_without_agent().await;
        });
        let _ = completion_rx.await;
        Ok(())
    }

    /// Navigate Pi's leaf via the injected `__pi_navigate_tree` extension
    /// command (official `ctx.navigateTree`).
    async fn navigate_session_tree(
        &self,
        entry_id: &str,
        summarize: bool,
        custom_instructions: Option<&str>,
    ) -> Result<Value, acp::Error> {
        let entry_id = entry_id.trim();
        if entry_id.is_empty() {
            return Err(acp::Error::invalid_params().data("tree entry id is empty"));
        }
        let mut args = entry_id.to_string();
        if summarize {
            args.push_str(" --summarize");
        }
        if let Some(instructions) = custom_instructions.map(str::trim).filter(|s| !s.is_empty()) {
            // Extension parses --instructions <rest-of-line>.
            args.push_str(" --instructions ");
            args.push_str(instructions);
        }
        self.run_bridge_command(NAVIGATE_TREE_COMMAND, &args).await?;

        // Leaf moved inside the same session file. Refresh adapter state only;
        // the pager issues session/load to clear scrollback and re-replay.
        let bootstrap = self.refresh().await.map_err(acp_internal)?;
        let tree = self.fetch_session_tree().await.map_err(acp_internal)?;
        Ok(json!({
            "sessionId": bootstrap.state.session_id,
            "leafId": tree.leaf_id,
            "cancelled": false,
        }))
    }

    async fn set_session_tree_label(
        &self,
        entry_id: &str,
        label: Option<&str>,
    ) -> Result<Value, acp::Error> {
        let entry_id = entry_id.trim();
        if entry_id.is_empty() {
            return Err(acp::Error::invalid_params().data("tree entry id is empty"));
        }
        let args = match label.map(str::trim).filter(|s| !s.is_empty()) {
            Some(text) => format!("{entry_id} {text}"),
            None => format!("{entry_id} --clear"),
        };
        self.run_bridge_command(LABEL_TREE_COMMAND, &args).await?;
        let tree = self.fetch_session_tree().await.map_err(acp_internal)?;
        Ok(json!({
            "leafId": tree.leaf_id,
            "entryId": entry_id,
            "label": label,
        }))
    }

    fn replace_bootstrap(&self, bootstrap: PiBootstrap) {
        let mut state = self.state.borrow_mut();
        state.acp_session_id = bootstrap.state.session_id.clone();
        state.model_map = bootstrap
            .models
            .iter()
            .cloned()
            .map(|model| (model_key(&model), model))
            .collect();
        state.bootstrap = bootstrap;
    }

    fn session_id(&self) -> acp::SessionId {
        acp::SessionId::new(self.state.borrow().acp_session_id.clone())
    }

    async fn send_update(&self, update: acp::SessionUpdate) {
        let mut notification = acp::SessionNotification::new(self.session_id(), update);
        if let Some(tokens) = self.state.borrow().last_context_tokens {
            let mut meta = acp::Meta::new();
            meta.insert("totalTokens".into(), json!(tokens));
            notification = notification.meta(Some(meta));
        }
        if let Err(error) = acp_send(notification, &self.client_tx).await {
            tracing::debug!(%error, "Grok pager closed while sending Pi session update");
        }
    }

    async fn send_update_for_session(
        &self,
        session_id: &str,
        update: acp::SessionUpdate,
        replay: bool,
        event_id: &str,
    ) {
        let mut notification = acp::SessionNotification::new(
            acp::SessionId::new(session_id.to_string()),
            update,
        );
        let mut meta = acp::Meta::new();
        if replay {
            meta.insert("isReplay".into(), Value::Bool(true));
        }
        meta.insert("eventId".into(), Value::String(event_id.to_string()));
        notification = notification.meta(Some(meta));
        if let Err(error) = acp_send(notification, &self.client_tx).await {
            tracing::debug!(%error, session_id, "Grok pager closed while sending Pi child session update");
        }
    }

    fn accept_subagent_bridge_sequence(&self, subagent_id: &str, sequence: u64, replay: bool) -> bool {
        let mut state = self.state.borrow_mut();
        let previous = state.subagent_bridge_sequences.get(subagent_id).copied();
        if !replay && previous.is_some_and(|last| sequence <= last) {
            return false;
        }
        state
            .subagent_bridge_sequences
            .entry(subagent_id.to_string())
            .and_modify(|last| *last = (*last).max(sequence))
            .or_insert(sequence);
        true
    }

    async fn handle_recap_bridge_message(&self, event: &Value) -> Result<bool> {
        let Some(projection) = parse_recap_message(event) else {
            return Ok(false);
        };
        let session_id = self.session_id().0.to_string();
        let notification = session_recap_notification(&session_id, &projection);
        self.send_ext_notification("x.ai/session/update", notification)
            .await;
        Ok(true)
    }

    async fn handle_subagent_bridge_message(&self, event: &Value) -> Result<bool> {
        if self
            .state
            .borrow_mut()
            .pending_subagent_bridge
            .defer_if_targeted(event)?
        {
            return Ok(true);
        }
        let root_session_id = self.session_id().0.to_string();
        let Some(projection) = parse_bridge_message(event, &root_session_id)? else {
            return Ok(false);
        };
        if !self.accept_subagent_bridge_sequence(
            &projection.subagent_id,
            projection.sequence,
            projection.replay,
        ) {
            return Ok(true);
        }
        let event_id = format!(
            "pi-grok-subagent:{}:{}",
            projection.subagent_id, projection.sequence
        );
        for operation in projection.operations {
            match operation {
                BridgeOperation::ParentTaskMetadata {
                    tool_call_id,
                    raw_input,
                } => {
                    self.send_update(acp::SessionUpdate::ToolCallUpdate(
                        acp::ToolCallUpdate::new(
                            acp::ToolCallId::new(tool_call_id),
                            acp::ToolCallUpdateFields::new()
                                .status(Some(acp::ToolCallStatus::InProgress))
                                .raw_input(Some(raw_input)),
                        ),
                    ))
                    .await;
                }
                BridgeOperation::ParentLifecycle(notification) => {
                    self.send_ext_notification("x.ai/session/update", notification)
                        .await;
                }
                BridgeOperation::ChildUpdate {
                    child_session_id,
                    update,
                } => {
                    self.send_update_for_session(
                        &child_session_id,
                        update,
                        projection.replay,
                        &event_id,
                    )
                    .await;
                }
            }
        }
        Ok(true)
    }

    /// Pull Pi's current context-window estimate and push it to the pager bar.
    ///
    /// Grok's context bar needs `_meta.totalTokens` on session updates plus the
    /// model window (`totalContextTokens`). Pi owns the estimate via
    /// `get_session_stats.contextUsage`.
    async fn refresh_context_usage(&self) {
        let data = match self
            .rpc
            .request(json!({ "type": "get_session_stats" }))
            .await
        {
            Ok(data) => data,
            Err(error) => {
                tracing::debug!(%error, "failed to fetch Pi session stats for context bar");
                return;
            }
        };
        let Some(tokens) = context_tokens_from_stats(&data) else {
            return;
        };
        let changed = {
            let mut state = self.state.borrow_mut();
            if state.last_context_tokens == Some(tokens) {
                false
            } else {
                state.last_context_tokens = Some(tokens);
                true
            }
        };
        if changed {
            // Empty chunk is a no-op in the tracker but still carries
            // `_meta.totalTokens` for confirm_context_used.
            self.send_update(acp::SessionUpdate::AgentMessageChunk(text_chunk("")))
                .await;
        }
    }

    fn note_context_tokens(&self, tokens: u64) {
        if tokens == 0 {
            return;
        }
        self.state.borrow_mut().last_context_tokens = Some(tokens);
    }

    async fn send_ext_notification(&self, method: &str, params: Value) {
        let Ok(raw) = serde_json::value::to_raw_value(&params) else {
            return;
        };
        let notification = acp::ExtNotification::new(method, raw.into());
        if let Err(error) = acp_send(notification, &self.client_tx).await {
            tracing::debug!(%error, method, "Grok pager closed while sending Pi UI notification");
        }
    }

    async fn send_ui_notification(&self, message: &str, kind: Option<&str>) {
        self.send_ext_notification(
            "pi/ui/notify",
            json!({ "message": message, "notifyType": kind }),
        )
        .await;
    }

    async fn handle_compaction_start(&self, event: &Value) {
        self.refresh_context_usage().await;
        let notification = (|| {
            let mut state = self.state.borrow_mut();
            state.compaction_started_at = Some(Instant::now());
            let tokens_used = state.last_context_tokens?;
            let context_window = state
                .bootstrap
                .state
                .model
                .as_ref()
                .and_then(|model| model.context_window)
                .filter(|window| *window > 0)?;
            Some(compaction_start_notification(
                &state.acp_session_id,
                event,
                tokens_used,
                context_window,
            ))
        })();
        if let Some(notification) = notification {
            self.send_ext_notification("x.ai/session/update", notification)
                .await;
        }
        self.send_status("compaction", Some("Compacting context…"))
            .await;
    }

    async fn handle_compaction_end(&self, event: &Value) {
        let (session_id, elapsed_ms) = {
            let mut state = self.state.borrow_mut();
            let elapsed_ms = state
                .compaction_started_at
                .take()
                .and_then(|started| started.elapsed().as_millis().try_into().ok());
            (state.acp_session_id.clone(), elapsed_ms)
        };
        if let Some(notification) = compaction_end_notification(&session_id, event, elapsed_ms) {
            self.send_ext_notification("x.ai/session/update", notification)
                .await;
        }
        self.send_status("compaction", None).await;
        if let Some(error) = string(event, &["errorMessage", "error"])
            && !error.is_empty()
        {
            self.send_ui_notification(error, Some("error")).await;
        }
    }

    /// Build Grok-native `x.ai/session/info` from Pi session stats.
    ///
    /// Mirrors the intent of the pi-context extension (`getContextUsage` +
    /// message-length estimate) but returns the ACP envelope that the pager's
    /// `ContextInfoBlock` already knows how to render — no second UI.
    async fn handle_session_info(&self) -> Result<acp::ExtResponse, acp::Error> {
        let stats = self
            .rpc
            .request(json!({ "type": "get_session_stats" }))
            .await
            .map_err(acp_internal)?;
        // Best-effort breakdown. Prefer live messages; fall back to branch entries
        // (session file shape) so empty/failed get_messages still yields a bar.
        let messages = match self.rpc.request(json!({ "type": "get_messages" })).await {
            Ok(value) if value.get("messages").and_then(Value::as_array).is_some_and(|m| !m.is_empty())
                || value.as_array().is_some_and(|m| !m.is_empty()) =>
            {
                Some(value)
            }
            _ => self
                .rpc
                .request(json!({ "type": "get_entries" }))
                .await
                .ok()
                .map(entries_to_messages_value),
        };
        let (session_id, model, cached_tokens) = {
            let state = self.state.borrow();
            (
                state.acp_session_id.clone(),
                state.bootstrap.state.model.clone(),
                state.last_context_tokens,
            )
        };
        let cwd = std::env::current_dir()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default();
        let response = build_session_info_response(
            &stats,
            messages.as_ref(),
            &session_id,
            &cwd,
            model.as_ref(),
            cached_tokens,
        );
        if let Some(used) = response
            .get("context")
            .and_then(|context| context.get("used"))
            .and_then(Value::as_u64)
            .filter(|&tokens| tokens > 0)
        {
            self.note_context_tokens(used);
        }
        ext_response(response).map_err(acp_internal)
    }

    async fn send_status(&self, key: &str, text: Option<&str>) {
        self.send_ext_notification(
            "pi/ui/status",
            json!({ "statusKey": key, "statusText": text }),
        )
        .await;
    }

    /// Publish Pi's authoritative queue as Grok's native shared-queue surface.
    async fn publish_queue_snapshot(&self) {
        let (session_id, snapshot) = {
            let state = self.state.borrow();
            (
                state.acp_session_id.clone(),
                state.queue_mirror.snapshot(),
            )
        };
        let steering_text = (snapshot.steering_count > 0)
            .then(|| format!("{} steering", snapshot.steering_count));
        let follow_up_text = (snapshot.follow_up_count > 0)
            .then(|| format!("{} follow-up", snapshot.follow_up_count));
        self.send_status("steering", steering_text.as_deref()).await;
        self.send_status("follow-up", follow_up_text.as_deref())
            .await;
        self.send_ext_notification(
            "x.ai/queue/changed",
            queue_changed_params(&session_id, &snapshot),
        )
        .await;
    }

    async fn apply_pi_queue_update(&self, event: &Value) {
        let steering = string_list(event.get("steering"));
        let follow_up = string_list(event.get("followUp"));
        {
            let mut state = self.state.borrow_mut();
            state
                .queue_mirror
                .apply_queue_update(&steering, &follow_up);
        }
        self.publish_queue_snapshot().await;
    }

    async fn rebroadcast_queue_mirror(&self) {
        self.publish_queue_snapshot().await;
    }

    async fn send_title(&self, title: Option<&str>) {
        let title = title
            .filter(|title| !title.trim().is_empty())
            .unwrap_or("Pi");
        self.send_ext_notification("pi/ui/title", json!({ "title": title }))
            .await;
    }

    async fn send_commands(&self, commands: &[PiCommand]) {
        self.send_update(acp::SessionUpdate::AvailableCommandsUpdate(
            acp::AvailableCommandsUpdate::new(command_catalog(commands)),
        ))
        .await;
    }

    async fn send_models(&self, bootstrap: &PiBootstrap) {
        let Some(models) = bootstrap.acp_models() else {
            return;
        };
        match serde_json::to_value(models) {
            Ok(value) => {
                self.send_ext_notification("x.ai/models/update", value)
                    .await;
            }
            Err(error) => tracing::warn!(%error, "failed to serialize Pi model state"),
        }
    }

    async fn publish_bootstrap(&self, bootstrap: &PiBootstrap) {
        self.send_commands(&bootstrap.commands).await;
        self.send_models(bootstrap).await;
        self.send_title(bootstrap.state.session_name.as_deref())
            .await;
    }

    async fn replay_history(&self) -> Result<()> {
        let data = self.rpc.request(json!({ "type": "get_messages" })).await?;
        for item in parse_messages(&data) {
            self.replay_history_item(item).await;
        }
        Ok(())
    }

    async fn replay_history_item(&self, item: PiHistoryItem) {
        let update = match item {
            PiHistoryItem::UserText(text) => acp::SessionUpdate::UserMessageChunk(text_chunk(text)),
            PiHistoryItem::UserImage { data, mime_type } => {
                acp::SessionUpdate::UserMessageChunk(content_chunk(acp::ContentBlock::Image(
                    acp::ImageContent::new(data, mime_type),
                )))
            }
            PiHistoryItem::AgentText(text) => {
                acp::SessionUpdate::AgentMessageChunk(text_chunk(text))
            }
            PiHistoryItem::AgentThought(text) => {
                acp::SessionUpdate::AgentThoughtChunk(text_chunk(text))
            }
            PiHistoryItem::ToolStart {
                id,
                name,
                arguments,
            } => {
                let arguments = normalize_tool_raw_input(&name, arguments);
                if let Some(args) = arguments.clone() {
                    self.state.borrow_mut().tool_args.insert(id.clone(), args);
                }
                acp::SessionUpdate::ToolCall(
                    acp::ToolCall::new(acp::ToolCallId::new(id), name.clone())
                        .kind(tool_kind(&name))
                        .status(acp::ToolCallStatus::InProgress)
                        .content(
                            edit_diff_content(&name, arguments.as_ref(), None).unwrap_or_default(),
                        )
                        .locations(Vec::new())
                        .raw_input(arguments),
                )
            }
            PiHistoryItem::ToolEnd {
                id,
                name,
                content,
                raw_output,
                is_error,
            } => {
                let mut raw = raw_output.unwrap_or(Value::Null);
                // History often stores `details` as raw_output and the body in
                // separate content blocks. Fold text into the payload so bash/read
                // projection still sees stdout / file text.
                if pi_result_text(&raw).is_empty() {
                    let text = content
                        .iter()
                        .filter_map(|item| match item {
                            PiToolContent::Text(text) => Some(text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    if !text.is_empty() {
                        raw = json!({ "content": [{ "type": "text", "text": text }] });
                    }
                }
                let args = self.state.borrow_mut().tool_args.remove(&id);
                let normalized = normalize_tool_raw_output(&name, args.as_ref(), &raw, is_error);
                let mut fields = acp::ToolCallUpdateFields::new()
                    .title(Some(name.clone()))
                    .status(Some(if is_error {
                        acp::ToolCallStatus::Failed
                    } else {
                        acp::ToolCallStatus::Completed
                    }))
                    .raw_output(Some(normalized));
                if tool_kind(&name) == acp::ToolKind::Edit {
                    fields = fields.content(edit_diff_content(&name, args.as_ref(), Some(&raw)));
                } else {
                    fields = fields.content(Some(history_tool_content(content)));
                }
                // Project todo-plugin snapshots onto the native TodoPane before
                // the tool card update so resume restores badge state.
                if let Some(plan) = plan_update_for_tool(&name, &raw, is_error) {
                    self.send_update(acp::SessionUpdate::Plan(plan)).await;
                }
                acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
                    acp::ToolCallId::new(id),
                    fields,
                ))
            }
        };
        self.send_update(update).await;
    }

    async fn handle_event(&self, event: Value) -> Result<()> {
        let event_type = event
            .get("type")
            .or_else(|| event.get("event"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        match event_type {
            "agent_start" => {
                for active in &mut self.state.borrow_mut().active_prompts {
                    active.agent_started = true;
                }
            }
            "agent_settled" => {
                self.refresh_context_usage().await;
                {
                    let mut state = self.state.borrow_mut();
                    state.queue_mirror.clear_running();
                }
                // Idle barrier: drop any stale runningPromptId so the pager can
                // drain local rows without waiting on a ghost running id.
                self.rebroadcast_queue_mirror().await;
                self.finish_prompts(acp::StopReason::EndTurn);
            }
            // `agent_end` is not the Pi idle barrier. Retry, compaction and
            // extension handlers can continue after it; `agent_settled` owns
            // ACP prompt completion.
            "agent_end" | "turn_start" => {}
            "turn_end" => self.refresh_context_usage().await,
            "message_start" => self.handle_message_start(&event),
            "message_update" => self.handle_message_update(&event).await,
            "message_end" => {
                if self.handle_recap_bridge_message(&event).await?
                    || self.handle_background_bash_bridge_message(&event).await?
                {
                    // Recap custom messages are display-only bridge traffic.
                } else if !self.handle_subagent_bridge_message(&event).await? {
                    self.handle_message_end(&event).await;
                }
            }
            "tool_execution_start" => self.handle_tool_start(&event).await,
            "tool_execution_update" => self.handle_tool_update(&event).await,
            "tool_execution_end" => self.handle_tool_end(&event).await,
            "extension_ui_request" => self.handle_extension_ui(event).await?,
            "extension_error" => {
                let message = event
                    .get("error")
                    .map(json_text)
                    .filter(|text| !text.is_empty())
                    .or_else(|| string(&event, &["message"]).map(ToOwned::to_owned))
                    .unwrap_or_else(|| "Pi extension error".to_string());
                self.send_ui_notification(&message, Some("error")).await;
            }
            "compaction_start" | "auto_compaction_start" => {
                self.handle_compaction_start(&event).await;
            }
            "compaction_end" | "auto_compaction_end" => {
                self.handle_compaction_end(&event).await;
            }
            "auto_retry_start" => {
                let attempt = event.get("attempt").and_then(Value::as_u64).unwrap_or(0);
                let maximum = event
                    .get("maxAttempts")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let delay_ms = event.get("delayMs").and_then(Value::as_u64).unwrap_or(0);
                let error =
                    string(&event, &["errorMessage", "message", "reason"]).unwrap_or_default();
                let mut text = if maximum > 0 {
                    format!("Retrying {attempt}/{maximum}")
                } else {
                    "Retrying".to_string()
                };
                if delay_ms > 0 {
                    text.push_str(&format!(" in {:.1}s", delay_ms as f64 / 1000.0));
                }
                if !error.is_empty() {
                    text.push_str(": ");
                    text.push_str(error);
                }
                self.send_status("retry", Some(&text)).await;
            }
            "auto_retry_end" => {
                self.send_status("retry", None).await;
                if event.get("success").and_then(Value::as_bool) == Some(false)
                    && let Some(error) = string(&event, &["finalError", "errorMessage"])
                {
                    self.send_ui_notification(error, Some("error")).await;
                }
            }
            "queue_update" => {
                // Pi emits full text arrays; mirror them into the native queue
                // pane so optimistic server rows confirm and dequeue.
                self.apply_pi_queue_update(&event).await;
            }
            "thinking_level_changed" | "session_info_changed" => match self.refresh().await {
                Ok(bootstrap) => self.publish_bootstrap(&bootstrap).await,
                Err(error) => {
                    tracing::warn!(%error, "failed to refresh Pi state after state change");
                }
            },
            "adapter_diagnostic" => {
                if let Some(message) = string(&event, &["message"]) {
                    self.send_ui_notification(message, Some("warning")).await;
                }
            }
            "adapter_process_exit" => {
                let message = string(&event, &["message"]).unwrap_or("Pi RPC process exited");
                self.send_ui_notification(message, Some("error")).await;
                self.finish_prompts(acp::StopReason::Cancelled);
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_message_start(&self, event: &Value) {
        if message_role(event) == Some("assistant") {
            self.state.borrow_mut().live_assistant = Some(StreamSeen::default());
        }
    }

    async fn handle_message_update(&self, event: &Value) {
        let (text, thought) = extract_delta(event);
        {
            let mut state = self.state.borrow_mut();
            let seen = state.live_assistant.get_or_insert_with(StreamSeen::default);
            seen.text |= !text.is_empty();
            seen.thought |= !thought.is_empty();
        }
        if !thought.is_empty() {
            self.send_update(acp::SessionUpdate::AgentThoughtChunk(text_chunk(thought)))
                .await;
        }
        if !text.is_empty() {
            self.send_update(acp::SessionUpdate::AgentMessageChunk(text_chunk(text)))
                .await;
        }
    }

    async fn handle_message_end(&self, event: &Value) {
        if message_role(event) != Some("assistant") {
            return;
        }
        let seen = self
            .state
            .borrow_mut()
            .live_assistant
            .take()
            .unwrap_or_default();
        let Some(message) = event.get("message") else {
            return;
        };
        // Prefer the assistant message's own usage for a low-latency bar update;
        // agent_settled still revalidates via get_session_stats.
        if let Some(tokens) = message
            .get("usage")
            .and_then(context_tokens_from_usage)
        {
            self.note_context_tokens(tokens);
        }
        let terminal_error = string(message, &["errorMessage", "error_message"])
            .filter(|error| !error.is_empty())
            .map(ToOwned::to_owned);
        for item in parse_messages(&json!({ "messages": [message] })) {
            match item {
                PiHistoryItem::AgentThought(text) if !seen.thought => {
                    self.send_update(acp::SessionUpdate::AgentThoughtChunk(text_chunk(text)))
                        .await;
                }
                PiHistoryItem::AgentText(text) if !seen.text => {
                    self.send_update(acp::SessionUpdate::AgentMessageChunk(text_chunk(text)))
                        .await;
                }
                _ => {}
            }
        }
        if seen.text
            && let Some(error) = terminal_error
        {
            self.send_ui_notification(&error, Some("error")).await;
        }
    }

    fn finish_prompts(&self, requested_reason: acp::StopReason) {
        let active_prompts = std::mem::take(&mut self.state.borrow_mut().active_prompts);
        for active in active_prompts {
            let reason = if active.cancelled {
                acp::StopReason::Cancelled
            } else {
                requested_reason.clone()
            };
            let _ = active.completion.send(PromptCompletion {
                reason,
                client_prompt_id: active.client_prompt_id,
            });
        }
    }

    fn remove_prompt(&self, id: u64) {
        let mut state = self.state.borrow_mut();
        if let Some(index) = state
            .active_prompts
            .iter()
            .position(|active| active.id == id)
        {
            state.active_prompts.remove(index);
        }
    }

    fn allocate_operation_id(&self) -> u64 {
        let mut state = self.state.borrow_mut();
        let id = state.next_prompt_id;
        state.next_prompt_id = state.next_prompt_id.wrapping_add(1).max(1);
        id
    }

    async fn probe_prompt_without_agent(&self) {
        // Pi acknowledges prompt preflight before its asynchronous event stream.
        // A short grace period lets a normal agent_start arrive. Extension
        // commands that complete without an agent run otherwise have no
        // agent_settled event, so get_state is the authoritative fallback.
        tokio::time::sleep(Duration::from_millis(40)).await;
        let should_probe = self
            .state
            .borrow()
            .active_prompts
            .iter()
            .any(|active| !active.agent_started);
        if !should_probe {
            return;
        }
        let Ok(value) = self.rpc.request(json!({ "type": "get_state" })).await else {
            return;
        };
        let pi_state = parse_state(&value);
        let should_finish = self
            .state
            .borrow()
            .active_prompts
            .iter()
            .any(|active| !active.agent_started)
            && !pi_state.is_streaming;
        if should_finish {
            self.finish_prompts(acp::StopReason::EndTurn);
        }
    }

    async fn execute_bash(
        &self,
        command: String,
        meta: Option<&acp::Meta>,
    ) -> Result<acp::PromptResponse, acp::Error> {
        let serial = self.allocate_operation_id();
        {
            let mut state = self.state.borrow_mut();
            if state.bash_running {
                return Err(
                    acp::Error::invalid_params().data("Pi already has a Bash command running")
                );
            }
            state.bash_running = true;
        }

        let call_id = meta
            .and_then(|meta| meta.get("promptId"))
            .and_then(Value::as_str)
            .filter(|id| !id.trim().is_empty())
            .map(|id| format!("pi-bash:{id}"))
            .unwrap_or_else(|| format!("pi-bash:{serial}"));
        let title = format!("$ {command}");
        self.send_update(acp::SessionUpdate::ToolCall(
            acp::ToolCall::new(acp::ToolCallId::new(call_id.clone()), title.clone())
                .kind(acp::ToolKind::Execute)
                .status(acp::ToolCallStatus::InProgress)
                .content(Vec::new())
                .locations(Vec::new())
                .raw_input(Some(json!({ "command": command.clone() }))),
        ))
        .await;

        let result = self
            .rpc
            .request(json!({ "type": "bash", "command": command }))
            .await;
        self.state.borrow_mut().bash_running = false;

        match result {
            Ok(result) => {
                let cancelled = result
                    .get("cancelled")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let exit_code = result.get("exitCode").and_then(Value::as_i64);
                let failed = cancelled || exit_code.is_some_and(|code| code != 0);
                let output = format_bash_result(&result);
                let raw_output = bash_tool_output(
                    &command,
                    &result,
                    failed && !cancelled,
                );
                self.send_update(acp::SessionUpdate::ToolCallUpdate(
                    acp::ToolCallUpdate::new(
                        acp::ToolCallId::new(call_id),
                        acp::ToolCallUpdateFields::new()
                            .title(Some(title))
                            .status(Some(if failed {
                                acp::ToolCallStatus::Failed
                            } else {
                                acp::ToolCallStatus::Completed
                            }))
                            .content(Some(vec![acp::ToolCallContent::from(
                                acp::ContentBlock::Text(acp::TextContent::new(output)),
                            )]))
                            .raw_output(Some(raw_output)),
                    ),
                ))
                .await;
                Ok(acp::PromptResponse::new(if cancelled {
                    acp::StopReason::Cancelled
                } else {
                    acp::StopReason::EndTurn
                }))
            }
            Err(error) => {
                self.send_update(acp::SessionUpdate::ToolCallUpdate(
                    acp::ToolCallUpdate::new(
                        acp::ToolCallId::new(call_id),
                        acp::ToolCallUpdateFields::new()
                            .title(Some(title))
                            .status(Some(acp::ToolCallStatus::Failed))
                            .content(Some(vec![acp::ToolCallContent::from(
                                acp::ContentBlock::Text(acp::TextContent::new(error.to_string())),
                            )])),
                    ),
                ))
                .await;
                Err(acp_internal(error))
            }
        }
    }

    async fn handle_tool_start(&self, event: &Value) {
        let id = string(event, &["toolCallId", "id"]).unwrap_or("pi-tool");
        let name = string(event, &["toolName", "name"]).unwrap_or("Tool");
        let args = normalize_tool_raw_input(
            name,
            event.get("args").or_else(|| event.get("input")).cloned(),
        );
        if let Some(args) = args.clone() {
            self.state
                .borrow_mut()
                .tool_args
                .insert(id.to_string(), args);
        }
        let content = edit_diff_content(name, args.as_ref(), None).unwrap_or_default();
        self.send_update(acp::SessionUpdate::ToolCall(
            acp::ToolCall::new(acp::ToolCallId::new(id.to_string()), name.to_string())
                .kind(tool_kind(name))
                .status(acp::ToolCallStatus::InProgress)
                .content(content)
                .locations(Vec::new())
                .raw_input(args),
        ))
        .await;
    }

    async fn handle_tool_update(&self, event: &Value) {
        let id = string(event, &["toolCallId", "id"]).unwrap_or("pi-tool");
        let output = event
            .get("partialResult")
            .or_else(|| event.get("result"))
            .cloned()
            .unwrap_or(Value::Null);
        let name = string(event, &["toolName", "name"]).unwrap_or_default();
        let args = normalize_tool_raw_input(
            name,
            event
                .get("args")
                .or_else(|| event.get("input"))
                .cloned()
                .or_else(|| self.state.borrow().tool_args.get(id).cloned()),
        );
        if let Some(args) = args.clone() {
            self.state
                .borrow_mut()
                .tool_args
                .insert(id.to_string(), args);
        }
        let raw_output = normalize_tool_raw_output(name, args.as_ref(), &output, false);
        let mut fields = acp::ToolCallUpdateFields::new()
            .status(Some(acp::ToolCallStatus::InProgress))
            .raw_output(Some(raw_output));
        if tool_kind(name) != acp::ToolKind::Edit {
            fields = fields.content(Some(tool_content(&output)));
        }
        self.send_update(acp::SessionUpdate::ToolCallUpdate(
            acp::ToolCallUpdate::new(acp::ToolCallId::new(id.to_string()), fields),
        ))
        .await;
    }

    async fn handle_tool_end(&self, event: &Value) {
        let id = string(event, &["toolCallId", "id"]).unwrap_or("pi-tool");
        let output = event.get("result").cloned().unwrap_or(Value::Null);
        let is_error = event.get("isError").and_then(Value::as_bool) == Some(true);
        let status = if is_error {
            acp::ToolCallStatus::Failed
        } else {
            acp::ToolCallStatus::Completed
        };
        let name = string(event, &["toolName", "name"]).unwrap_or_default();
        let args = normalize_tool_raw_input(
            name,
            event
                .get("args")
                .or_else(|| event.get("input"))
                .cloned()
                .or_else(|| self.state.borrow_mut().tool_args.remove(id)),
        );
        let raw_output = normalize_tool_raw_output(name, args.as_ref(), &output, is_error);
        let mut fields = acp::ToolCallUpdateFields::new()
            .status(Some(status))
            .raw_output(Some(raw_output));
        if tool_kind(name) == acp::ToolKind::Edit {
            fields = fields.content(edit_diff_content(name, args.as_ref(), Some(&output)));
        } else {
            fields = fields.content(Some(tool_content(&output)));
        }
        self.handle_background_bash_tool_end(name, id, args.as_ref(), &output)
            .await;
        // Live path: rpiv-todo (and future TodoSource plugins) publish a full
        // task snapshot under tool result details → native TodoPane via Plan.
        if let Some(plan) = plan_update_for_tool(name, &output, is_error) {
            self.send_update(acp::SessionUpdate::Plan(plan)).await;
        }
        self.send_update(acp::SessionUpdate::ToolCallUpdate(
            acp::ToolCallUpdate::new(acp::ToolCallId::new(id.to_string()), fields),
        ))
        .await;
    }

    async fn handle_background_bash_bridge_message(&self, event: &Value) -> Result<bool> {
        let Some(projection) = parse_background_bash_message(event) else {
            return Ok(false);
        };
        if let Some(output) = background_bash_output_update(&projection) {
            self.send_update(acp::SessionUpdate::ToolCallUpdate(
                acp::ToolCallUpdate::new(
                    acp::ToolCallId::new(output["toolCallId"].as_str().unwrap_or_default()),
                    acp::ToolCallUpdateFields::new()
                        .status(Some(acp::ToolCallStatus::InProgress))
                        .raw_output(Some(output["rawOutput"].clone())),
                ),
            ))
            .await;
        }
        let session_id = self.session_id().0.to_string();
        let (method, notification) = background_bash_notification(&session_id, &projection);
        self.send_ext_notification(method, notification).await;
        Ok(true)
    }

    async fn handle_background_bash_tool_end(
        &self,
        tool_name: &str,
        tool_call_id: &str,
        args: Option<&Value>,
        result: &Value,
    ) {
        let Some(projection) =
            parse_background_bash_tool_result(tool_name, tool_call_id, args, result)
        else {
            return;
        };
        let session_id = self.session_id().0.to_string();
        let (method, notification) = background_bash_notification(&session_id, &projection);
        self.send_ext_notification(method, notification).await;
    }

    async fn handle_extension_ui(&self, event: Value) -> Result<()> {
        let method = string(&event, &["method"])
            .unwrap_or_default()
            .to_ascii_lowercase();
        match method.as_str() {
            "notify" => {
                let message = string(&event, &["message"]).unwrap_or_default();
                let kind = string(&event, &["notifyType", "kind"]);
                self.send_ui_notification(message, kind).await;
            }
            "setstatus" => {
                let key = string(&event, &["statusKey", "key"]).unwrap_or("extension");
                let text = string(&event, &["statusText", "text"]);
                self.send_status(key, text.filter(|text| !text.is_empty()))
                    .await;
            }
            "setwidget" => {
                // Grok owns the sticky surface and ordering; the adapter only
                // forwards Pi's semantic widget payload.
                self.send_ext_notification("pi/ui/widget", event).await;
            }
            "settitle" => {
                if let Some(title) = string(&event, &["title"]) {
                    self.send_title(Some(title)).await;
                }
            }
            "set_editor_text" | "seteditortext" => {
                if let Some(text) = string(&event, &["text"]) {
                    self.send_ext_notification("pi/ui/editor_text", json!({ "text": text }))
                        .await;
                }
            }
            // Experimental Remote TUI: frames projected from Pi-process component host.
            "remote_tui_open" => {
                self.send_ext_notification(
                    "pi/ui/remote_tui",
                    json!({
                        "op": "open",
                        "id": event.get("id").cloned().unwrap_or(Value::Null),
                        "title": event.get("title").cloned().unwrap_or(Value::Null),
                        "width": event.get("width").cloned().unwrap_or(Value::Null),
                    }),
                )
                .await;
            }
            "remote_tui_frame" => {
                self.send_ext_notification(
                    "pi/ui/remote_tui",
                    json!({
                        "op": "frame",
                        "id": event.get("id").cloned().unwrap_or(Value::Null),
                        "lines": event.get("lines").cloned().unwrap_or(json!([])),
                        "width": event.get("width").cloned().unwrap_or(Value::Null),
                    }),
                )
                .await;
            }
            "remote_tui_close" => {
                self.send_ext_notification(
                    "pi/ui/remote_tui",
                    json!({
                        "op": "close",
                        "id": event.get("id").cloned().unwrap_or(Value::Null),
                    }),
                )
                .await;
            }
            "select" | "confirm" | "input" | "editor" => {
                let agent = self.clone();
                tokio::task::spawn_local(async move {
                    if let Err(error) = agent.ask_extension_question(event.clone()).await {
                        tracing::warn!(%error, "Pi extension question failed");
                        agent.respond_extension_cancelled(&event);
                        agent
                            .send_ui_notification(
                                &format!("Pi extension dialog failed: {error}"),
                                Some("error"),
                            )
                            .await;
                    }
                });
            }
            _ => self.respond_extension_cancelled(&event),
        }
        Ok(())
    }

    fn respond_extension_cancelled(&self, event: &Value) {
        if let Some(id) = event.get("id") {
            let _ = self.rpc.notify(json!({
                "type": "extension_ui_response",
                "id": id,
                "cancelled": true,
            }));
        }
    }

    async fn ask_extension_question(&self, event: Value) -> Result<()> {
        let id = event
            .get("id")
            .cloned()
            .ok_or_else(|| anyhow!("Pi extension UI request has no id"))?;
        let method = string(&event, &["method"])
            .unwrap_or_default()
            .to_ascii_lowercase();
        let title = string(&event, &["title", "message"]).unwrap_or("Pi extension");
        let mut options = Vec::new();
        if method == "select" {
            for option in event
                .get("options")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                options.push(json!({
                    "label": option,
                    "description": "",
                    "preview": null,
                    "id": null,
                }));
            }
        } else if method == "confirm" {
            options.push(json!({ "label": "Yes", "description": "", "preview": null, "id": null }));
            options.push(json!({ "label": "No", "description": "", "preview": null, "id": null }));
        }
        let mut question = if method == "confirm" {
            string(&event, &["message"]).unwrap_or(title).to_string()
        } else {
            title.to_string()
        };
        if method == "input"
            && let Some(placeholder) = string(&event, &["placeholder"])
            && !placeholder.is_empty()
        {
            question.push_str("\n\n");
            question.push_str(placeholder);
        }
        let initial_text = if method == "editor" {
            string(&event, &["prefill"]).unwrap_or_default()
        } else {
            ""
        };
        let tool_call_id = extension_tool_call_id(&id);
        let params = json!({
            "sessionId": self.session_id().0.to_string(),
            "toolCallId": tool_call_id.clone(),
            "questions": [{
                "question": question,
                "options": options,
                "multiSelect": false,
                "id": "pi-question",
            }],
            "mode": "default",
            "initialText": initial_text,
            "noFreeform": method == "select" || method == "confirm",
        });
        let raw = serde_json::value::to_raw_value(&params)?;
        let request = acp::ExtRequest::new("x.ai/ask_user_question", raw.into());
        let response = match extension_dialog_timeout(&event) {
            Some(duration) => {
                match tokio::time::timeout(duration, acp_send(request, &self.client_tx)).await {
                    Ok(response) => response.map_err(|error| anyhow!(error.to_string()))?,
                    Err(_) => {
                        // Pi resolves its own dialog promise on the same timeout but
                        // does not emit a cancellation event. Explicitly retract the
                        // native Grok QuestionView so it cannot remain as a zombie
                        // overlay after the extension has resumed.
                        self.send_ext_notification(
                            "pi/ui/cancel_interaction",
                            json!({ "toolCallId": tool_call_id }),
                        )
                        .await;
                        self.respond_extension_cancelled(&event);
                        return Ok(());
                    }
                }
            }
            None => acp_send(request, &self.client_tx)
                .await
                .map_err(|error| anyhow!(error.to_string()))?,
        };
        let outer: Value = serde_json::from_str(response.0.get())?;
        let result = outer.get("result").unwrap_or(&outer);
        if result.get("outcome").and_then(Value::as_str) == Some("cancelled") {
            self.rpc.notify(json!({
                "type": "extension_ui_response",
                "id": id,
                "cancelled": true,
            }))?;
            return Ok(());
        }
        let answer = extension_answer(&method, result).unwrap_or_default();
        let response = match method.as_str() {
            "confirm" => json!({
                "type": "extension_ui_response",
                "id": id,
                "confirmed": answer.eq_ignore_ascii_case("yes"),
            }),
            _ => json!({
                "type": "extension_ui_response",
                "id": id,
                "value": answer,
            }),
        };
        self.rpc.notify(response)?;
        Ok(())
    }
}

#[async_trait::async_trait(?Send)]
impl acp::Agent for PiAgent {
    async fn initialize(
        &self,
        _arguments: acp::InitializeRequest,
    ) -> Result<acp::InitializeResponse, acp::Error> {
        // Advertise session recap so Pager enables /recap + auto away-recap.
        // Actual generation is handled by the injected Pi extension bridge.
        let meta = json!({ "sessionRecap": true })
            .as_object()
            .cloned();
        Ok(acp::InitializeResponse::new(acp::ProtocolVersion::V1)
            .agent_capabilities(
                acp::AgentCapabilities::new()
                    .load_session(true)
                    .prompt_capabilities(
                        acp::PromptCapabilities::new()
                            .image(true)
                            .embedded_context(true),
                    ),
            )
            .agent_info(acp::Implementation::new("pi", env!("CARGO_PKG_VERSION")).title("Pi"))
            .meta(meta))
    }

    async fn authenticate(
        &self,
        _arguments: acp::AuthenticateRequest,
    ) -> Result<acp::AuthenticateResponse, acp::Error> {
        Ok(acp::AuthenticateResponse::new())
    }

    async fn new_session(
        &self,
        _arguments: acp::NewSessionRequest,
    ) -> Result<acp::NewSessionResponse, acp::Error> {
        let result = self
            .rpc
            .request(json!({ "type": "new_session" }))
            .await
            .map_err(acp_internal)?;
        let cancelled = result
            .get("cancelled")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let bootstrap = if cancelled {
            self.state.borrow().bootstrap.clone()
        } else {
            self.refresh().await.map_err(acp_internal)?
        };
        if !cancelled {
            self.state.borrow_mut().acp_session_id = bootstrap.state.session_id.clone();
        }
        self.publish_bootstrap(&bootstrap).await;
        let mut response = acp::NewSessionResponse::new(bootstrap.state.session_id.clone());
        if let Some(models) = bootstrap.acp_models() {
            response = response.models(Some(models));
        }
        Ok(response)
    }

    async fn load_session(
        &self,
        arguments: acp::LoadSessionRequest,
    ) -> Result<acp::LoadSessionResponse, acp::Error> {
        let requested = arguments.session_id.0.to_string();
        let active = self.state.borrow().bootstrap.state.session_id.clone();
        if requested != active {
            let session_path = self
                .state
                .borrow()
                .session_paths
                .get(&requested)
                .cloned()
                .ok_or_else(|| {
                    acp::Error::invalid_params().data(format!(
                        "Pi session {requested} is not in the local catalog"
                    ))
                })?;
            let result = self
                .switch_session(&session_path, &requested)
                .await
                .map_err(acp_internal)?;
            if result.cancelled {
                return Err(acp::Error::invalid_params().data("Pi session switch cancelled"));
            }
        }
        let bootstrap = self.state.borrow().bootstrap.clone();
        if requested != bootstrap.state.session_id {
            return Err(acp::Error::invalid_params().data(format!(
                "Pi switched to {}, not requested session {requested}",
                bootstrap.state.session_id
            )));
        }
        self.state.borrow_mut().acp_session_id = requested.clone();
        if let Err(error) = self.replay_history().await {
            self.state
                .borrow_mut()
                .pending_subagent_bridge
                .abandon(&requested);
            return Err(acp_internal(error));
        }
        self.publish_bootstrap(&bootstrap).await;
        self.refresh_context_usage().await;
        let replay_events = self
            .state
            .borrow_mut()
            .pending_subagent_bridge
            .commit_if_target(&requested)
            .map_err(acp_internal)?;
        for event in replay_events {
            self.handle_subagent_bridge_message(&event)
                .await
                .map_err(acp_internal)?;
        }
        let mut response = acp::LoadSessionResponse::new();
        if let Some(models) = bootstrap.acp_models() {
            response = response.models(Some(models));
        }
        Ok(response)
    }

    async fn set_session_mode(
        &self,
        _arguments: acp::SetSessionModeRequest,
    ) -> Result<acp::SetSessionModeResponse, acp::Error> {
        // Pi thinking is exposed through Grok's native model/effort surface,
        // not ACP session modes. No modes are advertised during initialize.
        Ok(acp::SetSessionModeResponse::new())
    }

    async fn prompt(
        &self,
        arguments: acp::PromptRequest,
    ) -> Result<acp::PromptResponse, acp::Error> {
        if let Some(command) = direct_bash_command(&arguments.prompt) {
            return self.execute_bash(command, arguments.meta.as_ref()).await;
        }

        let (message, images) = prompt_to_pi(&arguments.prompt);
        if message.trim().is_empty() && images.is_empty() {
            return Err(acp::Error::invalid_params().data("Pi prompt is empty"));
        }

        let client_prompt_id = arguments
            .meta
            .as_ref()
            .and_then(|meta| meta.get("promptId"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_string);

        let (completion_tx, completion_rx) = oneshot::channel();
        let (prompt_id, streaming_behavior) = {
            let mut state = self.state.borrow_mut();
            let already_active = !state.active_prompts.is_empty();
            let prompt_id = state.next_prompt_id;
            state.next_prompt_id = state.next_prompt_id.wrapping_add(1).max(1);
            let streaming_behavior =
                prompt_streaming_behavior(already_active, arguments.meta.as_ref());
            // Prefer the pager's client promptId so optimistic queue echoes
            // confirm by id (not only by text content).
            if let Some(lane) = streaming_behavior.and_then(queue_lane_for_behavior)
                && let Some(client_id) = client_prompt_id.as_deref()
            {
                state.queue_mirror.reserve(
                    client_id.to_string(),
                    message.clone(),
                    lane,
                );
            }
            state.active_prompts.push(ActivePrompt {
                id: prompt_id,
                client_prompt_id: client_prompt_id.clone(),
                completion: completion_tx,
                agent_started: false,
                cancelled: false,
            });
            (prompt_id, streaming_behavior)
        };
        let mut request = json!({ "type": "prompt", "message": message });
        if !images.is_empty() {
            request["images"] = Value::Array(images);
        }
        if let Some(streaming_behavior) = streaming_behavior {
            request["streamingBehavior"] = Value::String(streaming_behavior.to_string());
        }
        if let Err(error) = self.rpc.request(request).await {
            self.remove_prompt(prompt_id);
            return Err(acp_internal(error));
        }
        let probe = self.clone();
        tokio::task::spawn_local(async move {
            probe.probe_prompt_without_agent().await;
        });
        // Wait for agent_settled (or cancel). Mid-turn followUp/steer prompts
        // share this barrier with the primary turn; PromptResponse MUST carry
        // promptId so the pager discards non-current completions instead of
        // painting phantom "Worked for 0.0s" markers for each queued item.
        let completion = completion_rx.await.unwrap_or(PromptCompletion {
            reason: acp::StopReason::Cancelled,
            client_prompt_id: client_prompt_id.clone(),
        });
        Ok(prompt_response(
            completion.reason,
            completion
                .client_prompt_id
                .as_deref()
                .or(client_prompt_id.as_deref()),
        ))
    }

    async fn cancel(&self, _arguments: acp::CancelNotification) -> Result<(), acp::Error> {
        let command = {
            let mut state = self.state.borrow_mut();
            for active in &mut state.active_prompts {
                active.cancelled = true;
            }
            if state.bash_running {
                "abort_bash"
            } else {
                "abort"
            }
        };
        if let Err(error) = self.rpc.request(json!({ "type": command })).await {
            self.finish_prompts(acp::StopReason::Cancelled);
            return Err(acp_internal(error));
        }
        let probe = self.clone();
        tokio::task::spawn_local(async move {
            probe.probe_prompt_without_agent().await;
        });
        Ok(())
    }

    async fn set_session_model(
        &self,
        arguments: acp::SetSessionModelRequest,
    ) -> Result<acp::SetSessionModelResponse, acp::Error> {
        let model_id = arguments.model_id.0.to_string();
        let model = self
            .state
            .borrow()
            .model_map
            .get(&model_id)
            .cloned()
            .ok_or_else(|| {
                acp::Error::invalid_params().data(format!("unknown Pi model: {model_id}"))
            })?;
        let requested_effort = arguments
            .meta
            .as_ref()
            .and_then(|meta| meta.get("reasoningEffort"))
            .and_then(Value::as_str);
        let pi_effort = requested_effort
            .map(|effort| {
                model.pi_level_for_acp_effort(effort).ok_or_else(|| {
                    acp::Error::invalid_params().data(format!(
                        "Pi model {} does not support reasoning effort {effort}",
                        model.label
                    ))
                })
            })
            .transpose()?;
        let model_is_current = self
            .state
            .borrow()
            .bootstrap
            .state
            .model
            .as_ref()
            .is_some_and(|current| current.provider == model.provider && current.id == model.id);
        if !model_is_current {
            self.rpc
                .request(json!({
                    "type": "set_model",
                    "provider": model.provider,
                    "modelId": model.id,
                }))
                .await
                .map_err(acp_internal)?;
        }
        if let Some(level) = pi_effort {
            self.rpc
                .request(json!({
                    "type": "set_thinking_level",
                    "level": level,
                }))
                .await
                .map_err(acp_internal)?;
        }
        let bootstrap = self.refresh().await.map_err(acp_internal)?;
        self.publish_bootstrap(&bootstrap).await;
        Ok(acp::SetSessionModelResponse::new())
    }

    async fn ext_method(&self, arguments: acp::ExtRequest) -> Result<acp::ExtResponse, acp::Error> {
        match arguments.method.as_ref() {
            "x.ai/interject" => self.handle_steer_message(arguments.params.get()).await,
            "x.ai/terminal/background" => {
                self.handle_bash_background_request(arguments.params.get()).await
            }
            "x.ai/recap" => self.handle_recap_request(arguments.params.get()).await,
            "x.ai/compact_conversation" => {
                let params: Value =
                    serde_json::from_str(arguments.params.get()).unwrap_or_default();
                let mut request = json!({ "type": "compact" });
                if let Some(instructions) =
                    string(&params, &["customInstructions", "instructions", "context"])
                    && !instructions.trim().is_empty()
                {
                    request["customInstructions"] = Value::String(instructions.to_string());
                }
                let data = self.rpc.request(request).await.map_err(acp_internal)?;
                ext_response(data).map_err(acp_internal)
            }
            "pi/session/list" => {
                let params: Value =
                    serde_json::from_str(arguments.params.get()).unwrap_or_default();
                let cwd = string(&params, &["cwd"])
                    .filter(|cwd| !cwd.trim().is_empty())
                    .map(PathBuf::from)
                    .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
                let all = string(&params, &["scope"]) == Some("all");
                self.publish_session_catalog(cwd, all).await;
                ext_response(json!({})).map_err(acp_internal)
            }
            // Pi session entry tree (read-only projection of get_tree).
            "pi/session/tree" => {
                let tree = self.fetch_session_tree().await.map_err(acp_internal)?;
                ext_response(json!({
                    "leafId": tree.leaf_id,
                    "nodes": tree.rows.iter().map(|row| json!({
                        "id": row.id,
                        "parentId": row.parent_id,
                        "depth": row.depth,
                        "isLeaf": row.is_leaf,
                        "isCurrent": row.is_current,
                        "onActivePath": row.on_active_path,
                        "role": row.role,
                        "preview": row.preview,
                        "detail": row.detail,
                        "label": row.label,
                        "labelTimestamp": row.label_timestamp,
                        "entryType": row.entry_type,
                        "timestamp": row.timestamp,
                        "childIds": row.child_ids,
                        "hasText": row.has_text,
                    })).collect::<Vec<_>>(),
                }))
                .map_err(acp_internal)
            }
            // Navigate leaf via injected extension command → ctx.navigateTree.
            "pi/session/navigate_tree" => {
                let params: Value =
                    serde_json::from_str(arguments.params.get()).unwrap_or_default();
                let entry_id = string(&params, &["entryId", "id", "targetId"])
                    .map(str::trim)
                    .filter(|id| !id.is_empty())
                    .ok_or_else(|| acp::Error::invalid_params().data("entryId is required"))?;
                let summarize = params
                    .get("summarize")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let custom_instructions =
                    string(&params, &["customInstructions", "instructions"]);
                let data = self
                    .navigate_session_tree(entry_id, summarize, custom_instructions)
                    .await?;
                ext_response(data).map_err(acp_internal)
            }
            // Set/clear entry label via injected extension → ctx.setLabel.
            "pi/session/tree_label" => {
                let params: Value =
                    serde_json::from_str(arguments.params.get()).unwrap_or_default();
                let entry_id = string(&params, &["entryId", "id", "targetId"])
                    .map(str::trim)
                    .filter(|id| !id.is_empty())
                    .ok_or_else(|| acp::Error::invalid_params().data("entryId is required"))?;
                let clear = params
                    .get("clear")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let label = if clear {
                    None
                } else {
                    string(&params, &["label", "text"])
                };
                let data = self.set_session_tree_label(entry_id, label).await?;
                ext_response(data).map_err(acp_internal)
            }
            "x.ai/session/rename" => {
                let params: Value =
                    serde_json::from_str(arguments.params.get()).map_err(acp_internal)?;
                let title = string(&params, &["title", "name"])
                    .map(str::trim)
                    .filter(|title| !title.is_empty())
                    .ok_or_else(|| acp::Error::invalid_params().data("session title is empty"))?;
                self.rpc
                    .request(json!({ "type": "set_session_name", "name": title }))
                    .await
                    .map_err(acp_internal)?;
                if let Ok(bootstrap) = self.refresh().await {
                    self.publish_bootstrap(&bootstrap).await;
                } else {
                    self.send_title(Some(title)).await;
                }
                ext_response(json!({})).map_err(acp_internal)
            }
            // Grok `/context` and context-bar click fetch this; map Pi
            // get_session_stats (+ message estimate) into native ContextInfo.
            "x.ai/session/info" => self.handle_session_info().await,
            "x.ai/subagent/cancel" => {
                let params: Value = serde_json::from_str(arguments.params.get()).map_err(acp_internal)?;
                let subagent_id = string(&params, &["subagentId", "subagent_id"])
                    .map(str::trim)
                    .filter(|id| !id.is_empty())
                    .ok_or_else(|| acp::Error::invalid_params().data("subagentId is required"))?;
                self.run_bridge_command(SUBAGENT_CANCEL_COMMAND, subagent_id)
                    .await?;
                ext_response(json!({
                    "subagentId": subagent_id,
                    "cancelled": true,
                    "outcome": "cancelled",
                }))
                .map_err(acp_internal)
            }
            method => Err(acp::Error::new(
                acp::ErrorCode::MethodNotFound.into(),
                format!("Method not found: {method}"),
            )),
        }
    }

    async fn ext_notification(&self, arguments: acp::ExtNotification) -> Result<(), acp::Error> {
        match arguments.method.as_ref() {
            "pi/extension_command" => {
                let params: Value = serde_json::from_str(arguments.params.get()).unwrap_or_default();
                let command = string(&params, &["command"])
                    .map(str::trim)
                    .filter(|command| command.starts_with('/'));
                if let Some(command) = command {
                    if let Err(error) = self
                        .rpc
                        .request(json!({ "type": "prompt", "message": command }))
                        .await
                    {
                        tracing::warn!(%error, "failed to invoke Pi extension command");
                    }
                } else {
                    tracing::warn!("ignored malformed Pi extension command notification");
                }
                Ok(())
            }
            // Experimental Remote TUI: keys go to extension host via keyfile
            // (no Pi source patch / no custom stdin RPC types).
            "pi/ui/remote_tui/input" => {
                let params: Value =
                    serde_json::from_str(arguments.params.get()).unwrap_or_default();
                if let Some(data) = string(&params, &["data"]) {
                    if let Err(error) = append_remote_tui_key_event(json!({
                        "op": "input",
                        "data": data,
                    })) {
                        tracing::debug!(%error, "remote_tui keyfile input failed");
                    }
                }
                Ok(())
            }
            "pi/ui/remote_tui/cancel" => {
                if let Err(error) = append_remote_tui_key_event(json!({ "op": "cancel" })) {
                    tracing::debug!(%error, "remote_tui keyfile cancel failed");
                }
                Ok(())
            }
            "x.ai/queue/remove" | "x.ai/queue/clear" | "x.ai/queue/edit" | "x.ai/queue/reorder" => {
                // Pi RPC does not expose per-item queue mutation / clearQueue.
                // Rebroadcast the authoritative Pi mirror so the pager cannot
                // keep a client-only ghost removal, and surface the boundary.
                if matches!(
                    arguments.method.as_ref(),
                    "x.ai/queue/remove" | "x.ai/queue/clear" | "x.ai/queue/edit"
                ) {
                    self.send_ui_notification(
                        "Pi queue is read-mirrored; remove/edit/clear are not supported over RPC",
                        Some("warning"),
                    )
                    .await;
                }
                self.rebroadcast_queue_mirror().await;
                Ok(())
            }
            "x.ai/queue/interject" => {
                // Promote by text via steer (same lane as x.ai/interject).
                // Pi cannot drop a single follow-up row, so a follow-up item may
                // still deliver later — documented adapter boundary.
                let params: Value =
                    serde_json::from_str(arguments.params.get()).unwrap_or_default();
                let id = string(&params, &["id"]).unwrap_or_default();
                let text = {
                    let state = self.state.borrow();
                    state.queue_mirror.text_for_id(id).map(str::to_string)
                };
                let text =
                    text.or_else(|| string(&params, &["newText", "text"]).map(str::to_string));
                if let Some(text) = text.filter(|text| !text.trim().is_empty()) {
                    let _ = self
                        .rpc
                        .request(json!({
                            "type": "prompt",
                            "message": text,
                            "streamingBehavior": "steer",
                        }))
                        .await;
                }
                self.rebroadcast_queue_mirror().await;
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

impl PiAgent {
    async fn handle_bash_background_request(
        &self,
        params_raw: &str,
    ) -> Result<acp::ExtResponse, acp::Error> {
        let params: Value = serde_json::from_str(params_raw).map_err(acp_internal)?;
        let tool_call_id = string(&params, &["terminalId", "toolCallId"])
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| acp::Error::invalid_params().data("terminalId is required"))?;
        let control_meta = self
            .bash_control_meta
            .as_deref()
            .ok_or_else(|| acp::Error::invalid_params().data("Pi Bash background control is disabled"))?;
        append_bash_background_control(control_meta, tool_call_id).map_err(acp_internal)?;
        ext_response(json!({ "accepted": true, "terminalId": tool_call_id }))
            .map_err(acp_internal)
    }

    /// Fire-and-forget session recap via injected `__pi_grok_recap` extension.
    ///
    /// Params: `{ sessionId?, auto?, model? }`. Language is taken from process locale.
    async fn handle_recap_request(&self, params_raw: &str) -> Result<acp::ExtResponse, acp::Error> {
        let params: Value = serde_json::from_str(params_raw).unwrap_or_else(|_| json!({}));
        let auto = params.get("auto").and_then(Value::as_bool).unwrap_or(false);
        let model = string(&params, &["model", "modelId", "recapModel"])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned);
        let language = system_language_tag();
        let payload = json!({
            "auto": auto,
            "model": model,
            "language": language,
        });
        let args = payload.to_string();
        // Extension emits custom message asynchronously; adapter projects it.
        // Await preflight so extension errors surface before we ack.
        self.run_bridge_command(RECAP_COMMAND, &args).await?;
        ext_response(json!({ "ok": true, "auto": auto })).map_err(acp_internal)
    }

    async fn handle_steer_message(&self, params_raw: &str) -> Result<acp::ExtResponse, acp::Error> {
        let params: Value = serde_json::from_str(params_raw).map_err(acp_internal)?;
        let blocks = params
            .get("content")
            .cloned()
            .and_then(|value| serde_json::from_value::<Vec<acp::ContentBlock>>(value).ok());
        let (message, images) = if let Some(blocks) = blocks.as_deref() {
            prompt_to_pi(blocks)
        } else {
            (
                string(&params, &["text"]).unwrap_or_default().to_string(),
                Vec::new(),
            )
        };
        if message.trim().is_empty() && images.is_empty() {
            return Err(acp::Error::invalid_params().data("Pi interjection is empty"));
        }
        if let Some(client_id) = string(&params, &["interjectionId", "promptId"])
            .map(str::trim)
            .filter(|id| !id.is_empty())
        {
            self.state.borrow_mut().queue_mirror.reserve(
                client_id.to_string(),
                message.clone(),
                QueueLane::Steering,
            );
        }
        let mut request = json!({
            "type": "prompt",
            "message": message,
            "streamingBehavior": "steer",
        });
        if !images.is_empty() {
            request["images"] = Value::Array(images);
        }
        let data = self.rpc.request(request).await.map_err(acp_internal)?;
        ext_response(data).map_err(acp_internal)
    }
}

fn build_model_catalog(
    models: &[PiModel],
    current: Option<&PiModel>,
    thinking_level: &str,
) -> (IndexMap<acp::ModelId, acp::ModelInfo>, Option<acp::ModelId>) {
    let mut available = IndexMap::new();
    for model in models {
        let id = acp::ModelId::new(model_key(model));
        let mut meta = serde_json::Map::new();
        if !model.provider.is_empty() {
            meta.insert("provider".into(), json!(model.provider));
        }
        meta.insert("modelId".into(), json!(model.id));
        if let Some(tokens) = model.context_window {
            meta.insert("totalContextTokens".into(), json!(tokens));
        }
        if let Some(tokens) = model.max_tokens {
            meta.insert("maxTokens".into(), json!(tokens));
        }
        if let Some(api) = model.api.as_ref() {
            meta.insert("api".into(), json!(api));
        }
        if let Some(base_url) = model.base_url.as_ref() {
            meta.insert("baseUrl".into(), json!(base_url));
        }
        meta.insert("acceptsImages".into(), json!(model.accepts_images));
        meta.insert("reasoning".into(), json!(model.reasoning));
        if !model.input.is_empty() {
            meta.insert(
                "inputModalities".into(),
                Value::Array(model.input.iter().cloned().map(Value::String).collect()),
            );
        }
        if model.cost_input.is_some()
            || model.cost_output.is_some()
            || model.cost_cache_read.is_some()
            || model.cost_cache_write.is_some()
        {
            let mut cost = serde_json::Map::new();
            if let Some(v) = model.cost_input {
                cost.insert("input".into(), json!(v));
            }
            if let Some(v) = model.cost_output {
                cost.insert("output".into(), json!(v));
            }
            if let Some(v) = model.cost_cache_read {
                cost.insert("cacheRead".into(), json!(v));
            }
            if let Some(v) = model.cost_cache_write {
                cost.insert("cacheWrite".into(), json!(v));
            }
            meta.insert("cost".into(), Value::Object(cost));
        }
        let reasoning_efforts = model_reasoning_efforts(model);
        if !reasoning_efforts.is_empty() {
            meta.insert("supportsReasoningEffort".into(), json!(true));
            meta.insert(
                "reasoningEffort".into(),
                json!(pi_effort_to_acp(thinking_level)),
            );
            meta.insert("reasoningEfforts".into(), Value::Array(reasoning_efforts));
        }
        let description = model_catalog_description(model);
        let mut info = acp::ModelInfo::new(id.clone(), model.label.clone()).meta(Some(meta));
        if !description.is_empty() {
            info = info.description(description);
        }
        available.insert(id, info);
    }
    let current = current.map(|model| acp::ModelId::new(model_key(model)));
    (available, current)
}

/// Compact right-side detail for the native model picker.
/// Provider lives on the left row (`id [provider]`); this side mirrors
/// model-selector-x metadata: context / max-out / protocol / input / cost.
fn model_catalog_description(model: &PiModel) -> String {
    let mut parts = Vec::new();
    if let Some(tokens) = model.context_window {
        parts.push(format!("ctx {}", format_token_count(tokens)));
    }
    if let Some(tokens) = model.max_tokens {
        parts.push(format!("out {}", format_token_count(tokens)));
    }
    if let Some(api) = model.api.as_deref().and_then(format_protocol_short) {
        parts.push(format!("api {api}"));
    }
    let input = format_input_short(&model.input, model.accepts_images);
    if !input.is_empty() {
        parts.push(format!("in {input}"));
    }
    if model.reasoning {
        parts.push("⚡".into());
    }
    if let Some(cost) = format_cost_short(model) {
        parts.push(cost);
    }
    // Fallback when we only know the provider (no numeric metadata yet).
    if parts.is_empty() && !model.provider.is_empty() {
        return format!("[{}]", model.provider);
    }
    parts.join(" · ")
}

fn format_token_count(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        let millions = tokens as f64 / 1_000_000.0;
        if millions.fract() == 0.0 {
            format!("{}M", millions as u64)
        } else {
            format!("{millions:.1}M")
        }
    } else if tokens >= 1_000 {
        let thousands = tokens as f64 / 1_000.0;
        if thousands.fract() == 0.0 {
            format!("{}k", thousands as u64)
        } else {
            format!("{thousands:.0}k")
        }
    } else {
        tokens.to_string()
    }
}

fn format_protocol_short(api: &str) -> Option<&'static str> {
    match api {
        "openai-responses" | "openai-codex-responses" => Some("resp"),
        "openai-completions" => Some("comp"),
        "anthropic-messages" => Some("anth"),
        "google-generative-ai" => Some("goog"),
        _ => None,
    }
}

fn format_input_short(input: &[String], accepts_images: bool) -> String {
    if input.is_empty() {
        return if accepts_images {
            "txt+img".into()
        } else {
            "txt".into()
        };
    }
    let mut parts = Vec::new();
    if input.iter().any(|m| m.eq_ignore_ascii_case("text")) {
        parts.push("txt");
    }
    if input.iter().any(|m| m.eq_ignore_ascii_case("image")) || accepts_images {
        parts.push("img");
    }
    if input.iter().any(|m| m.eq_ignore_ascii_case("audio")) {
        parts.push("aud");
    }
    if parts.is_empty() {
        "txt".into()
    } else {
        parts.join("+")
    }
}

fn format_cost_short(model: &PiModel) -> Option<String> {
    let input = model.cost_input.unwrap_or(0.0);
    let output = model.cost_output.unwrap_or(0.0);
    if input == 0.0 && output == 0.0 {
        // Only claim free when cost fields were present.
        if model.cost_input.is_some() || model.cost_output.is_some() {
            return Some("free".into());
        }
        return None;
    }
    Some(format!(
        "${} / ${}",
        format_cost_num(input),
        format_cost_num(output)
    ))
}

fn format_cost_num(value: f64) -> String {
    if value == 0.0 {
        "0".into()
    } else if value < 0.01 {
        format!("{value:.3}")
    } else if value < 1.0 {
        format!("{value:.2}")
    } else if (value - value.round()).abs() < f64::EPSILON {
        format!("{}", value.round() as i64)
    } else if value < 10.0 {
        format!("{value:.1}")
    } else {
        format!("{}", value.round() as i64)
    }
}

/// Internal bridge commands injected by grok-pi; never advertised to slash UI.
const NAVIGATE_TREE_COMMAND: &str = "__pi_navigate_tree";
const LABEL_TREE_COMMAND: &str = "__pi_tree_label";
const SUBAGENT_CANCEL_COMMAND: &str = "__pi_grok_subagent_cancel";
const RECAP_COMMAND: &str = "__pi_grok_recap";

fn is_bridge_command(name: &str) -> bool {
    name.eq_ignore_ascii_case(NAVIGATE_TREE_COMMAND)
        || name.eq_ignore_ascii_case(LABEL_TREE_COMMAND)
        || name.eq_ignore_ascii_case(SUBAGENT_CANCEL_COMMAND)
        || name.eq_ignore_ascii_case(RECAP_COMMAND)
}

/// Process locale for recap body language. Prefers LC_ALL → LC_MESSAGES → LANG.
fn system_language_tag() -> Option<String> {
    for key in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(value) = std::env::var(key) {
            let trimmed = value.trim();
            if !trimmed.is_empty() && trimmed != "C" && trimmed != "POSIX" {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

fn command_catalog(commands: &[PiCommand]) -> Vec<acp::AvailableCommand> {
    // The adapter reports Pi's command catalog verbatim (normalized and
    // deduplicated). Grok's native CommandRegistry owns collision policy with
    // pager-local commands such as /help, /model, and /compact.
    let mut seen = HashSet::new();
    commands
        .iter()
        .filter_map(|command| {
            let name = command.name.trim().trim_start_matches('/');
            if name.is_empty() || is_bridge_command(name) || !seen.insert(name.to_ascii_lowercase())
            {
                return None;
            }
            let description = if command.description.trim().is_empty() {
                if command.source.trim().is_empty() {
                    "Pi command".to_string()
                } else {
                    format!("Pi {} command", command.source)
                }
            } else {
                command.description.clone()
            };
            let mut available = acp::AvailableCommand::new(name.to_string(), description);
            if !command.source.trim().is_empty() {
                available = available.meta(serde_json::Map::from_iter([(
                    "piCommandSource".to_string(),
                    Value::String(command.source.clone()),
                )]));
            }
            Some(available)
        })
        .collect()
}

fn model_key(model: &PiModel) -> String {
    if model.provider.is_empty() {
        model.id.clone()
    } else {
        format!("{}::{}", model.provider, model.id)
    }
}

fn catalog_session_dir(state: &PiState, configured_dir: &Path) -> PathBuf {
    state
        .session_file
        .as_deref()
        .map(Path::new)
        .and_then(Path::parent)
        .filter(|directory| !directory.starts_with(configured_dir))
        .map(Path::to_path_buf)
        .unwrap_or_else(|| configured_dir.to_path_buf())
}

fn content_chunk(content: acp::ContentBlock) -> acp::ContentChunk {
    acp::ContentChunk::new(content)
}

fn text_chunk(text: impl Into<String>) -> acp::ContentChunk {
    content_chunk(acp::ContentBlock::Text(acp::TextContent::new(text)))
}

fn message_role(event: &Value) -> Option<&str> {
    event
        .get("message")
        .and_then(|message| string(message, &["role", "type"]))
}

fn model_reasoning_efforts(model: &PiModel) -> Vec<Value> {
    let mut efforts = Vec::new();
    for level in &model.thinking_levels {
        let entry = match level.as_str() {
            "off" => Some(json!({ "id": "off", "value": "none", "label": "Off" })),
            "minimal" => Some(json!({ "id": "minimal", "value": "minimal", "label": "Minimal" })),
            "low" => Some(json!({ "id": "low", "value": "low", "label": "Low" })),
            "medium" => Some(json!({ "id": "medium", "value": "medium", "label": "Medium" })),
            "high" => Some(json!({ "id": "high", "value": "high", "label": "High" })),
            "xhigh" | "max" => {
                if efforts.iter().any(|value: &Value| {
                    value.get("value").and_then(Value::as_str) == Some("xhigh")
                }) {
                    None
                } else {
                    Some(json!({ "id": "xhigh", "value": "xhigh", "label": "Extra high" }))
                }
            }
            _ => None,
        };
        if let Some(entry) = entry {
            efforts.push(entry);
        }
    }
    efforts
}

fn pi_effort_to_acp(level: &str) -> &str {
    match level.to_ascii_lowercase().as_str() {
        "off" | "none" => "none",
        "minimal" => "minimal",
        "low" => "low",
        "high" => "high",
        "xhigh" | "max" => "xhigh",
        _ => "medium",
    }
}

fn compaction_start_notification(
    session_id: &str,
    event: &Value,
    tokens_used: u64,
    context_window: u64,
) -> Value {
    let percentage = tokens_used
        .saturating_mul(100)
        .checked_div(context_window)
        .unwrap_or(100)
        .min(100) as u8;
    json!({
        "sessionId": session_id,
        "update": {
            "sessionUpdate": "auto_compact_started",
            "tokens_used": tokens_used,
            "context_window": context_window,
            "percentage": percentage,
            "reason": string(event, &["reason"]).unwrap_or("unknown"),
        }
    })
}

fn compaction_end_notification(
    session_id: &str,
    event: &Value,
    elapsed_ms: Option<i64>,
) -> Option<Value> {
    let update = if let Some(error) =
        string(event, &["errorMessage", "error"]).filter(|error| !error.is_empty())
    {
        json!({ "sessionUpdate": "auto_compact_failed", "error": error })
    } else if event.get("aborted").and_then(Value::as_bool) == Some(true) {
        json!({
            "sessionUpdate": "auto_compact_cancelled",
            "reason": string(event, &["reason"]).unwrap_or("Compaction cancelled"),
        })
    } else {
        let result = event.get("result")?;
        let tokens_after = result.get("estimatedTokensAfter").and_then(Value::as_u64)?;
        json!({
            "sessionUpdate": "auto_compact_completed",
            "tokens_before": result.get("tokensBefore").and_then(Value::as_u64),
            "tokens_after": tokens_after,
            "elapsed_ms": elapsed_ms,
            "summary_preview": result.get("summary").and_then(Value::as_str),
        })
    };
    Some(json!({ "sessionId": session_id, "update": update }))
}

/// Send a Pager foreground-to-background request to the injected Bash extension.
///
/// The per-process metadata path is minted by the composition binary and passed
/// to both Pi and this adapter. The extension publishes only live foreground
/// tool IDs, so the adapter cannot create a background task for an arbitrary
/// or already completed tool call.
fn append_bash_background_control(meta_path: &Path, tool_call_id: &str) -> Result<()> {
    use std::fs::OpenOptions;
    use std::io::Write;

    let meta: Value = serde_json::from_str(&std::fs::read_to_string(meta_path)?)?;
    let active = meta
        .get("activeToolCallIds")
        .and_then(Value::as_array)
        .is_some_and(|ids| ids.iter().any(|id| id.as_str() == Some(tool_call_id)));
    if !active {
        bail!("Pi Bash tool is not promotable: {tool_call_id}");
    }
    let control_path = meta
        .get("controlPath")
        .and_then(Value::as_str)
        .filter(|path| !path.trim().is_empty())
        .ok_or_else(|| anyhow!("Pi Bash control metadata missing controlPath"))?;
    let mut file = OpenOptions::new().append(true).open(control_path)?;
    writeln!(
        file,
        "{}",
        serde_json::to_string(&json!({ "op": "background", "toolCallId": tool_call_id }))?
    )?;
    Ok(())
}

/// Experimental Remote TUI: extension host watches a keyfile under tmp.
/// Meta written by the injected extension: `{id, keysPath}`.
fn append_remote_tui_key_event(event: Value) -> Result<()> {
    use std::fs::OpenOptions;
    use std::io::Write;

    let meta_path = std::env::temp_dir().join("pi-grok-remote-tui-active.json");
    if !meta_path.exists() {
        bail!("remote_tui meta missing ({})", meta_path.display());
    }
    let meta: Value = serde_json::from_str(&std::fs::read_to_string(&meta_path)?)?;
    let keys_path = meta
        .get("keysPath")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("remote_tui meta missing keysPath"))?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(keys_path)?;
    writeln!(file, "{}", serde_json::to_string(&event)?)?;
    Ok(())
}

fn extension_tool_call_id(id: &Value) -> String {
    let id = id
        .as_str()
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| id.to_string());
    format!("pi-extension-ui:{id}")
}

fn extension_dialog_timeout(event: &Value) -> Option<Duration> {
    event
        .get("timeout")
        .and_then(Value::as_u64)
        .filter(|milliseconds| *milliseconds > 0)
        .map(Duration::from_millis)
}

fn selected_answer(value: &Value) -> Option<String> {
    let answers = value.get("answers").and_then(Value::as_object)?;
    for answer in answers.values() {
        if let Some(text) = answer.as_str() {
            return Some(text.to_string());
        }
        if let Some(text) = answer
            .as_array()
            .and_then(|items| items.first())
            .and_then(Value::as_str)
        {
            return Some(text.to_string());
        }
    }
    None
}

fn annotated_answer(value: &Value) -> Option<String> {
    let annotations = value.get("annotations").and_then(Value::as_object)?;
    for annotation in annotations.values() {
        if let Some(notes) = annotation.get("notes").and_then(Value::as_str) {
            return Some(notes.to_string());
        }
    }
    None
}

/// Translate Grok QuestionView's response into the value Pi expects.
///
/// Freeform rows are represented by the native question component as the
/// selected option `Other`, with the actual editor text under
/// `annotations.<question>.notes`. Pi input/editor must therefore prefer notes;
/// select/confirm must prefer the selected option.
fn extension_answer(method: &str, value: &Value) -> Option<String> {
    let direct = || {
        value
            .get("value")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
    };
    match method {
        "input" | "editor" => annotated_answer(value)
            .or_else(|| selected_answer(value))
            .or_else(direct),
        _ => selected_answer(value)
            .or_else(|| annotated_answer(value))
            .or_else(direct),
    }
}

fn ext_response(value: Value) -> Result<acp::ExtResponse> {
    let raw = serde_json::value::to_raw_value(&json!({ "result": value }))?;
    Ok(acp::ExtResponse::new(raw.into()))
}

fn acp_internal(error: impl std::fmt::Display) -> acp::Error {
    acp::Error::internal_error().data(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_subagent_bridge_defers_only_the_target_session_replay() {
        let replay = json!({
            "message": {
                "role": "custom",
                "customType": "pi-grok-subagent/v1",
                "details": { "parentSessionId": "session-next" },
            },
        });
        let other_session = json!({
            "message": {
                "role": "custom",
                "customType": "pi-grok-subagent/v1",
                "details": { "parentSessionId": "session-other" },
            },
        });
        let mut pending = PendingSubagentBridge::default();
        pending.begin("session-next").expect("begin transition");
        assert!(pending.defer_if_targeted(&replay).expect("defer target replay"));
        assert!(!pending
            .defer_if_targeted(&other_session)
            .expect("leave unrelated event live"));
        assert_eq!(
            pending
                .commit_if_target("session-next")
                .expect("commit")
                .len(),
            1
        );
        assert!(pending
            .commit_if_target("session-next")
            .expect("no transition")
            .is_empty());
    }

    #[test]
    fn pending_subagent_bridge_discards_replay_when_transition_is_cancelled() {
        let replay = json!({
            "message": {
                "role": "custom",
                "customType": "pi-grok-subagent/v1",
                "details": { "parentSessionId": "session-next" },
            },
        });
        let mut pending = PendingSubagentBridge::default();
        pending.begin("session-next").expect("begin transition");
        assert!(pending.defer_if_targeted(&replay).expect("defer target replay"));
        pending.abandon("session-next");
        assert!(pending.events.is_empty());
        assert!(pending.target_session_id.is_none());
    }

    #[test]
    fn appends_background_control_only_for_an_active_tool() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let control_path = directory.path().join("control.jsonl");
        let meta_path = directory.path().join("control.json");
        std::fs::write(&control_path, "").expect("control file");
        std::fs::write(
            &meta_path,
            json!({
                "controlPath": control_path,
                "activeToolCallIds": ["tool-1"],
            })
            .to_string(),
        )
        .expect("metadata file");

        append_bash_background_control(&meta_path, "tool-1").expect("append control event");
        assert_eq!(
            std::fs::read_to_string(&control_path).expect("read control file"),
            "{\"op\":\"background\",\"toolCallId\":\"tool-1\"}\n"
        );
        assert!(append_bash_background_control(&meta_path, "tool-2").is_err());
    }

    #[test]
    fn session_file_discovers_a_settings_configured_session_directory() {
        let fallback = Path::new("/home/user/.pi/agent/sessions");
        let state = PiState {
            session_file: Some("/data/pi-sessions/current.jsonl".to_string()),
            ..PiState::default()
        };
        assert_eq!(
            catalog_session_dir(&state, fallback),
            PathBuf::from("/data/pi-sessions")
        );

        let default_state = PiState {
            session_file: Some("/home/user/.pi/agent/sessions/project/current.jsonl".to_string()),
            ..PiState::default()
        };
        assert_eq!(catalog_session_dir(&default_state, fallback), fallback);
    }

    #[test]
    fn model_catalog_includes_provider_and_detail_description() {
        let models = vec![PiModel {
            provider: "anthropic".into(),
            id: "claude-haiku-4-5".into(),
            label: "Claude Haiku 4.5".into(),
            context_window: Some(200_000),
            max_tokens: Some(64_000),
            api: Some("anthropic-messages".into()),
            base_url: Some("https://api.anthropic.com".into()),
            reasoning: true,
            accepts_images: true,
            input: vec!["text".into(), "image".into()],
            cost_input: Some(1.0),
            cost_output: Some(5.0),
            cost_cache_read: Some(0.1),
            cost_cache_write: Some(1.25),
            thinking_levels: vec!["off".into(), "low".into(), "medium".into(), "high".into()],
        }];
        let (available, current) = build_model_catalog(&models, models.first(), "medium");
        assert!(current.is_some());
        let id = acp::ModelId::new("anthropic::claude-haiku-4-5");
        let info = available.get(&id).expect("catalog entry");
        assert_eq!(info.name, "Claude Haiku 4.5");
        let description = info.description.as_deref().unwrap_or("");
        assert!(!description.contains("[anthropic]"), "provider stays on left: {description}");
        assert!(description.contains("ctx 200k"), "{description}");
        assert!(description.contains("out 64k"), "{description}");
        assert!(description.contains("api anth"), "{description}");
        assert!(description.contains("in txt+img"), "{description}");
        assert!(description.contains("⚡"), "{description}");
        assert!(description.contains("$1 / $5"), "{description}");
        let meta = info.meta.as_ref().expect("meta");
        assert_eq!(meta.get("provider").and_then(|v| v.as_str()), Some("anthropic"));
        assert_eq!(meta.get("modelId").and_then(|v| v.as_str()), Some("claude-haiku-4-5"));
        assert_eq!(meta.get("api").and_then(|v| v.as_str()), Some("anthropic-messages"));
        assert_eq!(meta.get("totalContextTokens").and_then(|v| v.as_u64()), Some(200_000));
        assert_eq!(meta.get("maxTokens").and_then(|v| v.as_u64()), Some(64_000));
    }

    #[test]
    fn command_catalog_is_pi_owned_and_deduplicated() {
        let commands = vec![
            PiCommand {
                name: "/review".into(),
                description: "Review changes".into(),
                source: "extension".into(),
            },
            PiCommand {
                name: "REVIEW".into(),
                description: "Duplicate".into(),
                source: "prompt".into(),
            },
            PiCommand {
                name: "brief".into(),
                description: String::new(),
                source: "skill".into(),
            },
            PiCommand {
                name: NAVIGATE_TREE_COMMAND.into(),
                description: "internal".into(),
                source: "extension".into(),
            },
            PiCommand {
                name: LABEL_TREE_COMMAND.into(),
                description: "internal".into(),
                source: "extension".into(),
            },
        ];
        let serialized = serde_json::to_value(command_catalog(&commands)).unwrap();
        let text = serialized.to_string();
        assert_eq!(text.matches("review").count(), 1);
        assert!(text.contains("Review changes"));
        assert!(text.contains("brief"));
        assert!(text.contains("Pi skill command"));
        assert!(text.contains("piCommandSource"));
        assert!(text.contains("extension"));
        assert!(!text.contains(NAVIGATE_TREE_COMMAND));
        assert!(!text.contains(LABEL_TREE_COMMAND));
    }

    #[test]
    fn pi_input_and_editor_prefer_native_freeform_annotations() {
        let result = json!({
            "answers": { "pi-question": ["Other"] },
            "annotations": { "pi-question": { "notes": "typed in Grok PromptWidget" } },
            "value": "fallback",
        });
        assert_eq!(
            extension_answer("input", &result).as_deref(),
            Some("typed in Grok PromptWidget")
        );
        assert_eq!(
            extension_answer("editor", &result).as_deref(),
            Some("typed in Grok PromptWidget")
        );
    }

    #[test]
    fn pi_select_and_confirm_prefer_native_selected_option() {
        let result = json!({
            "answers": { "pi-question": ["Yes"] },
            "annotations": { "pi-question": { "notes": "ignored freeform" } },
            "value": "fallback",
        });
        assert_eq!(extension_answer("select", &result).as_deref(), Some("Yes"));
        assert_eq!(extension_answer("confirm", &result).as_deref(), Some("Yes"));
    }

    #[test]
    fn pi_extension_timeout_is_milliseconds_and_zero_means_no_timeout() {
        assert_eq!(
            extension_dialog_timeout(&json!({ "timeout": 2500 })),
            Some(Duration::from_millis(2500))
        );
        assert_eq!(extension_dialog_timeout(&json!({ "timeout": 0 })), None);
        assert_eq!(extension_dialog_timeout(&json!({})), None);
    }

    #[test]
    fn extension_tool_call_ids_are_stable_and_namespaced() {
        assert_eq!(
            extension_tool_call_id(&json!("dialog-7")),
            "pi-extension-ui:dialog-7"
        );
        assert_eq!(extension_tool_call_id(&json!(17)), "pi-extension-ui:17");
    }

    #[test]
    fn compaction_events_project_to_native_session_updates() {
        let start = compaction_start_notification(
            "session-1",
            &json!({ "reason": "threshold" }),
            85_000,
            100_000,
        );
        assert_eq!(start["update"]["sessionUpdate"], "auto_compact_started");
        assert_eq!(start["update"]["percentage"], 85);

        let success = compaction_end_notification(
            "session-1",
            &json!({
                "result": {
                    "tokensBefore": 100_000,
                    "estimatedTokensAfter": 20_000,
                    "summary": "Retained recent work"
                }
            }),
            Some(500),
        )
        .expect("success projection");
        assert_eq!(success["sessionId"], "session-1");
        assert_eq!(success["update"]["sessionUpdate"], "auto_compact_completed");
        assert_eq!(success["update"]["tokens_before"], 100_000);
        assert_eq!(success["update"]["tokens_after"], 20_000);
        assert_eq!(success["update"]["elapsed_ms"], 500);

        let failure = compaction_end_notification(
            "session-1",
            &json!({ "errorMessage": "compaction failed" }),
            None,
        )
        .expect("failure projection");
        assert_eq!(failure["update"]["sessionUpdate"], "auto_compact_failed");

        let cancelled = compaction_end_notification(
            "session-1",
            &json!({ "aborted": true, "reason": "user" }),
            None,
        )
        .expect("cancelled projection");
        assert_eq!(
            cancelled["update"]["sessionUpdate"],
            "auto_compact_cancelled"
        );
    }
}
