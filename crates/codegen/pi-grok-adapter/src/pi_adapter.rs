use crate::{
    model::{
        PiCommand, PiHistoryItem, PiModel, PiSessionSwitch, PiState, PiToolContent, extract_delta,
        json_text, parse_commands, parse_messages, parse_models, parse_session_switch, parse_state,
        scan_local_sessions, string,
    },
    pi_rpc::PiRpc,
};
use agent_client_protocol as acp;
use anyhow::{Result, anyhow};
use indexmap::IndexMap;
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    rc::Rc,
    path::{Path, PathBuf},
    time::Duration,
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
    completion: oneshot::Sender<acp::StopReason>,
    agent_started: bool,
    cancelled: bool,
}

#[derive(Debug, Clone, Copy, Default)]
struct StreamSeen {
    text: bool,
    thought: bool,
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
}

#[derive(Clone)]
pub struct PiAgent {
    rpc: PiRpc,
    client_tx: mpsc::UnboundedSender<AcpClientMessage>,
    state: Rc<RefCell<AdapterState>>,
}

impl PiAgent {
    pub fn new(
        rpc: PiRpc,
        client_tx: mpsc::UnboundedSender<AcpClientMessage>,
        bootstrap: PiBootstrap,
        session_dir: PathBuf,
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
    pub async fn publish_session_catalog(&self) {
        let session_dir = {
            let state = self.state.borrow();
            catalog_session_dir(&state.bootstrap.state, &state.session_dir)
        };
        let sessions = scan_local_sessions(&session_dir);
        let paths = sessions
            .iter()
            .map(|session| (session.id.clone(), session.path.clone()))
            .collect();
        self.state.borrow_mut().session_paths = paths;
        self.send_ext_notification(
            "pi/ui/session_catalog",
            json!({
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
    pub async fn switch_session(&self, session_path: &Path) -> Result<PiSessionSwitch> {
        let response = self
            .rpc
            .request(json!({
                "type": "switch_session",
                "sessionPath": session_path,
            }))
            .await?;
        let result = parse_session_switch(&response);
        if result.cancelled {
            return Ok(result);
        }
        let bootstrap = PiBootstrap::load(&self.rpc).await?;
        self.replace_bootstrap(bootstrap);
        Ok(result)
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
        let notification = acp::SessionNotification::new(self.session_id(), update);
        if let Err(error) = acp_send(notification, &self.client_tx).await {
            tracing::debug!(%error, "Grok pager closed while sending Pi session update");
        }
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

    async fn send_status(&self, key: &str, text: Option<&str>) {
        self.send_ext_notification(
            "pi/ui/status",
            json!({ "statusKey": key, "statusText": text }),
        )
        .await;
    }

    async fn send_title(&self, title: Option<&str>) {
        let title = title.filter(|title| !title.trim().is_empty()).unwrap_or("Pi");
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
                self.send_ext_notification("x.ai/models/update", value).await;
            }
            Err(error) => tracing::warn!(%error, "failed to serialize Pi model state"),
        }
    }

    async fn publish_bootstrap(&self, bootstrap: &PiBootstrap) {
        self.send_commands(&bootstrap.commands).await;
        self.send_models(bootstrap).await;
        self.send_title(bootstrap.state.session_name.as_deref()).await;
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
            PiHistoryItem::UserText(text) => {
                acp::SessionUpdate::UserMessageChunk(text_chunk(text))
            }
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
            } => acp::SessionUpdate::ToolCall(
                acp::ToolCall::new(acp::ToolCallId::new(id), name.clone())
                    .kind(tool_kind(&name))
                    .status(acp::ToolCallStatus::InProgress)
                    .content(edit_diff_content(&name, arguments.as_ref()).unwrap_or_default())
                    .locations(Vec::new())
                    .raw_input(arguments),
            ),
            PiHistoryItem::ToolEnd {
                id,
                name,
                content,
                raw_output,
                is_error,
            } => acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
                acp::ToolCallId::new(id),
                acp::ToolCallUpdateFields::new()
                    .title(Some(name))
                    .status(Some(if is_error {
                        acp::ToolCallStatus::Failed
                    } else {
                        acp::ToolCallStatus::Completed
                    }))
                    .content(Some(history_tool_content(content)))
                    .raw_output(raw_output),
            )),
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
            "agent_settled" => self.finish_prompts(acp::StopReason::EndTurn),
            // `agent_end` is not the Pi idle barrier. Retry, compaction and
            // extension handlers can continue after it; `agent_settled` owns
            // ACP prompt completion.
            "agent_end" | "turn_start" | "turn_end" => {}
            "message_start" => self.handle_message_start(&event),
            "message_update" => self.handle_message_update(&event).await,
            "message_end" => self.handle_message_end(&event).await,
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
                self.send_status("compaction", Some("Compacting context…"))
                    .await;
            }
            "compaction_end" | "auto_compaction_end" => {
                self.send_status("compaction", None).await;
                if let Some(error) = string(&event, &["errorMessage", "error"])
                    && !error.is_empty()
                {
                    self.send_ui_notification(error, Some("error")).await;
                }
            }
            "auto_retry_start" => {
                let attempt = event.get("attempt").and_then(Value::as_u64).unwrap_or(0);
                let maximum = event
                    .get("maxAttempts")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let delay_ms = event.get("delayMs").and_then(Value::as_u64).unwrap_or(0);
                let error = string(&event, &["errorMessage", "message", "reason"])
                    .unwrap_or_default();
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
                let steering = event
                    .get("steering")
                    .and_then(Value::as_array)
                    .map_or(0, Vec::len);
                let follow_up = event
                    .get("followUp")
                    .and_then(Value::as_array)
                    .map_or(0, Vec::len);
                let pending = steering
                    .saturating_add(follow_up)
                    .max(
                        event
                            .get("pendingMessageCount")
                            .or_else(|| event.get("pending"))
                            .and_then(Value::as_u64)
                            .unwrap_or(0) as usize,
                    );
                let text = (pending > 0).then(|| format!("{pending} queued"));
                self.send_status("queue", text.as_deref()).await;
            }
            "thinking_level_changed" | "session_info_changed" => {
                match self.refresh().await {
                    Ok(bootstrap) => self.publish_bootstrap(&bootstrap).await,
                    Err(error) => {
                        tracing::warn!(%error, "failed to refresh Pi state after state change");
                    }
                }
            }
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
            let _ = active.completion.send(reason);
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
                            .raw_output(Some(result)),
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
        let args = event
            .get("args")
            .or_else(|| event.get("input"))
            .cloned();
        let content = edit_diff_content(name, args.as_ref()).unwrap_or_default();
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
        let mut fields = acp::ToolCallUpdateFields::new()
            .status(Some(acp::ToolCallStatus::InProgress))
            .raw_output(Some(output.clone()));
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
        let status = if event.get("isError").and_then(Value::as_bool) == Some(true) {
            acp::ToolCallStatus::Failed
        } else {
            acp::ToolCallStatus::Completed
        };
        let name = string(event, &["toolName", "name"]).unwrap_or_default();
        let mut fields = acp::ToolCallUpdateFields::new()
            .status(Some(status))
            .raw_output(Some(output.clone()));
        if tool_kind(name) != acp::ToolKind::Edit {
            fields = fields.content(Some(tool_content(&output)));
        }
        self.send_update(acp::SessionUpdate::ToolCallUpdate(
            acp::ToolCallUpdate::new(acp::ToolCallId::new(id.to_string()), fields),
        ))
        .await;
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
                self.send_status(key, text.filter(|text| !text.is_empty())).await;
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
            options.push(
                json!({ "label": "Yes", "description": "", "preview": null, "id": null }),
            );
            options.push(
                json!({ "label": "No", "description": "", "preview": null, "id": null }),
            );
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
            Some(duration) => match tokio::time::timeout(
                duration,
                acp_send(request, &self.client_tx),
            )
            .await
            {
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
            },
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
        Ok(acp::InitializeResponse::new(acp::ProtocolVersion::V1)
            .agent_capabilities(
                acp::AgentCapabilities::new().load_session(true).prompt_capabilities(
                    acp::PromptCapabilities::new()
                        .image(true)
                        .embedded_context(true),
                ),
            )
            .agent_info(acp::Implementation::new("pi", env!("CARGO_PKG_VERSION")).title("Pi")))
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
                    acp::Error::invalid_params()
                        .data(format!("Pi session {requested} is not in the local catalog"))
                })?;
            let result = self.switch_session(&session_path).await.map_err(acp_internal)?;
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
        self.state.borrow_mut().acp_session_id = requested;
        self.replay_history().await.map_err(acp_internal)?;
        self.publish_bootstrap(&bootstrap).await;
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

        let (completion_tx, completion_rx) = oneshot::channel();
        let (prompt_id, streaming_behavior) = {
            let mut state = self.state.borrow_mut();
            let already_active = !state.active_prompts.is_empty();
            let prompt_id = state.next_prompt_id;
            state.next_prompt_id = state.next_prompt_id.wrapping_add(1).max(1);
            state.active_prompts.push(ActivePrompt {
                id: prompt_id,
                completion: completion_tx,
                agent_started: false,
                cancelled: false,
            });
            (
                prompt_id,
                prompt_streaming_behavior(already_active, arguments.meta.as_ref()),
            )
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
        let reason = completion_rx.await.unwrap_or(acp::StopReason::Cancelled);
        Ok(acp::PromptResponse::new(reason))
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
        self.rpc
            .request(json!({
                "type": "set_model",
                "provider": model.provider,
                "modelId": model.id,
            }))
            .await
            .map_err(acp_internal)?;
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

    async fn ext_method(
        &self,
        arguments: acp::ExtRequest,
    ) -> Result<acp::ExtResponse, acp::Error> {
        match arguments.method.as_ref() {
            "x.ai/interject" => {
                let params: Value =
                    serde_json::from_str(arguments.params.get()).map_err(acp_internal)?;
                let blocks = params
                    .get("content")
                    .cloned()
                    .and_then(|value| {
                        serde_json::from_value::<Vec<acp::ContentBlock>>(value).ok()
                    });
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
            "x.ai/compact_conversation" => {
                let params: Value = serde_json::from_str(arguments.params.get()).unwrap_or_default();
                let mut request = json!({ "type": "compact" });
                if let Some(instructions) = string(
                    &params,
                    &["customInstructions", "instructions", "context"],
                ) && !instructions.trim().is_empty()
                {
                    request["customInstructions"] = Value::String(instructions.to_string());
                }
                let data = self.rpc.request(request).await.map_err(acp_internal)?;
                ext_response(data).map_err(acp_internal)
            }
            "pi/session/list" => {
                self.publish_session_catalog().await;
                ext_response(json!({})).map_err(acp_internal)
            }
            "x.ai/session/rename" => {
                let params: Value = serde_json::from_str(arguments.params.get())
                    .map_err(acp_internal)?;
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
            method => Err(acp::Error::new(
                acp::ErrorCode::MethodNotFound.into(),
                format!("Method not found: {method}"),
            )),
        }
    }

    async fn ext_notification(
        &self,
        _arguments: acp::ExtNotification,
    ) -> Result<(), acp::Error> {
        Ok(())
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
        if let Some(tokens) = model.context_window {
            meta.insert("totalContextTokens".into(), json!(tokens));
        }
        meta.insert("acceptsImages".into(), json!(model.accepts_images));
        let reasoning_efforts = model_reasoning_efforts(model);
        if !reasoning_efforts.is_empty() {
            meta.insert("supportsReasoningEffort".into(), json!(true));
            meta.insert(
                "reasoningEffort".into(),
                json!(pi_effort_to_acp(thinking_level)),
            );
            meta.insert("reasoningEfforts".into(), Value::Array(reasoning_efforts));
        }
        let info = acp::ModelInfo::new(id.clone(), model.label.clone()).meta(Some(meta));
        available.insert(id, info);
    }
    let current = current.map(|model| acp::ModelId::new(model_key(model)));
    (available, current)
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
            if name.is_empty() || !seen.insert(name.to_ascii_lowercase()) {
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
            Some(acp::AvailableCommand::new(name.to_string(), description))
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

fn history_tool_content(content: Vec<PiToolContent>) -> Vec<acp::ToolCallContent> {
    content
        .into_iter()
        .map(|item| match item {
            PiToolContent::Text(text) => acp::ToolCallContent::from(acp::ContentBlock::Text(
                acp::TextContent::new(text),
            )),
            PiToolContent::Image { data, mime_type } => {
                acp::ToolCallContent::from(acp::ContentBlock::Image(acp::ImageContent::new(
                    data, mime_type,
                )))
            }
        })
        .collect()
}

fn tool_content(value: &Value) -> Vec<acp::ToolCallContent> {
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

/// Convert Pi's edit/write input contract into ACP's native diff payload.
///
/// The Pager's Edit card and viewer intentionally render only `Diff` content;
/// ordinary text results do not provide the old/new source needed for a hunk.
fn edit_diff_content(
    tool_name: &str,
    args: Option<&Value>,
) -> Option<Vec<acp::ToolCallContent>> {
    if tool_kind(tool_name) != acp::ToolKind::Edit {
        return None;
    }
    let args = args?;
    let path = string(args, &["path", "filePath", "file_path", "target_file"])?;
    let new_text = string(args, &["newText", "new_text", "content"])?;
    let old_text = string(args, &["oldText", "old_text"]).map(ToOwned::to_owned);
    Some(vec![acp::ToolCallContent::Diff(
        acp::Diff::new(path, new_text.to_owned()).old_text(old_text),
    )])
}

fn tool_kind(name: &str) -> acp::ToolKind {
    let name = name.to_ascii_lowercase();
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

fn direct_bash_command(blocks: &[acp::ContentBlock]) -> Option<String> {
    blocks.iter().find_map(|block| {
        let acp::ContentBlock::Text(text) = block else {
            return None;
        };
        text.meta
            .as_ref()
            .and_then(|meta| meta.get("bash_command"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|command| !command.is_empty())
            .map(ToOwned::to_owned)
    })
}

fn prompt_streaming_behavior(
    already_active: bool,
    meta: Option<&acp::Meta>,
) -> Option<&'static str> {
    if !already_active {
        return None;
    }
    let send_now = meta
        .and_then(|meta| meta.get("sendNow"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Some(if send_now { "steer" } else { "followUp" })
}

fn format_bash_result(result: &Value) -> String {
    let mut text = result
        .get("output")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let mut notes = Vec::new();
    if result.get("cancelled").and_then(Value::as_bool) == Some(true) {
        notes.push("Command cancelled".to_string());
    } else if let Some(exit_code) = result.get("exitCode").and_then(Value::as_i64) {
        notes.push(format!("Exit code: {exit_code}"));
    }
    if result.get("truncated").and_then(Value::as_bool) == Some(true) {
        let suffix = result
            .get("fullOutputPath")
            .and_then(Value::as_str)
            .map(|path| format!("Output truncated; full output: {path}"))
            .unwrap_or_else(|| "Output truncated".to_string());
        notes.push(suffix);
    }
    if !notes.is_empty() {
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&notes.join("\n"));
    }
    if text.is_empty() {
        "Command completed with no output".to_string()
    } else {
        text
    }
}

fn prompt_to_pi(blocks: &[acp::ContentBlock]) -> (String, Vec<Value>) {
    let mut parts = Vec::new();
    let mut images = Vec::new();
    for block in blocks {
        match block {
            acp::ContentBlock::Text(text) => parts.push(text.text.clone()),
            acp::ContentBlock::Image(image) => images.push(json!({
                "type": "image",
                "data": image.data,
                "mimeType": image.mime_type,
            })),
            acp::ContentBlock::ResourceLink(link) => {
                parts.push(format!("[resource] {}", link.uri));
            }
            acp::ContentBlock::Resource(resource) => {
                parts.push(json_text(
                    &serde_json::to_value(resource).unwrap_or(Value::Null),
                ));
            }
            _ => {}
        }
    }
    (parts.join("\n\n"), images)
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
    fn session_file_discovers_a_settings_configured_session_directory() {
        let fallback = Path::new("/home/user/.pi/agent/sessions");
        let state = PiState {
            session_file: Some("/data/pi-sessions/current.jsonl".to_string()),
            ..PiState::default()
        };
        assert_eq!(catalog_session_dir(&state, fallback), PathBuf::from("/data/pi-sessions"));

        let default_state = PiState {
            session_file: Some("/home/user/.pi/agent/sessions/project/current.jsonl".to_string()),
            ..PiState::default()
        };
        assert_eq!(catalog_session_dir(&default_state, fallback), fallback);
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
        ];
        let serialized = serde_json::to_value(command_catalog(&commands)).unwrap();
        let text = serialized.to_string();
        assert_eq!(text.matches("review").count(), 1);
        assert!(text.contains("Review changes"));
        assert!(text.contains("brief"));
        assert!(text.contains("Pi skill command"));
    }

    #[test]
    fn grok_direct_bash_meta_maps_to_pi_bash() {
        let mut meta = acp::Meta::new();
        meta.insert("bash_command".into(), json!("git status"));
        let blocks = vec![acp::ContentBlock::Text(
            acp::TextContent::new("!git status").meta(Some(meta)),
        )];
        assert_eq!(direct_bash_command(&blocks).as_deref(), Some("git status"));
    }

    #[test]
    fn grok_queue_modes_map_to_pi_streaming_behavior() {
        assert_eq!(prompt_streaming_behavior(false, None), None);
        assert_eq!(prompt_streaming_behavior(true, None), Some("followUp"));

        let mut meta = acp::Meta::new();
        meta.insert("sendNow".into(), Value::Bool(true));
        assert_eq!(prompt_streaming_behavior(true, Some(&meta)), Some("steer"));
    }

    #[test]
    fn bash_result_is_presented_in_native_tool_card_text() {
        let text = format_bash_result(&json!({
            "output": "ok",
            "exitCode": 0,
            "cancelled": false,
            "truncated": true,
            "fullOutputPath": "/tmp/pi-bash.log",
        }));
        assert!(text.contains("ok"));
        assert!(text.contains("Exit code: 0"));
        assert!(text.contains("/tmp/pi-bash.log"));
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
        )
        .expect("edit input must become a diff");
        let acp::ToolCallContent::Diff(diff) = &edit[0] else {
            panic!("edit input must produce ACP Diff content");
        };
        assert_eq!(diff.path.to_string_lossy(), "README.md");
        assert_eq!(diff.old_text.as_deref(), Some("before\n"));
        assert_eq!(diff.new_text, "after\n");

        let write = edit_diff_content(
            "write",
            Some(&json!({ "path": "README.md", "content": "new file\n" })),
        )
        .expect("write input must become a diff");
        let acp::ToolCallContent::Diff(diff) = &write[0] else {
            panic!("write input must produce ACP Diff content");
        };
        assert_eq!(diff.old_text, None);
        assert_eq!(diff.new_text, "new file\n");
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
        assert_eq!(
            extension_tool_call_id(&json!(17)),
            "pi-extension-ui:17"
        );
    }
}
