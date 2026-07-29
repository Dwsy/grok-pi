use crate::{
    background_bash_bridge::{
        background_bash_notification, background_bash_output_update, parse_background_bash_message,
        parse_background_bash_tool_result,
    },
    btw_bridge::parse_btw_message,
    context_projection::{
        build_session_info_response, context_tokens_from_stats, context_tokens_from_usage,
        entries_to_messages_value, parse_context_breakdown,
    },
    goal_host::{GoalControl, GoalHost},
    loop_host,
    model::{
        PiCommand, PiHistoryItem, PiModel, PiReplayEntry, PiSessionSwitch, PiSessionTree, PiState,
        PiToolContent, extract_delta, json_text, parse_commands, parse_messages, parse_models,
        parse_session_switch, parse_session_tree, parse_state, scan_local_sessions,
        scan_local_sessions_for_cwd, string, tree_entry_editor_text,
    },
    pi_rpc::PiRpc,
    pi_workflow_backend::{
        BridgeCommandRequest, BridgeCommandTx, WORKFLOW_CANCEL_COMMAND, WORKFLOW_SPAWN_COMMAND,
    },
    prompt_bridge::{
        direct_bash_command, format_bash_result, prompt_response, prompt_streaming_behavior,
        prompt_to_pi, queue_lane_for_behavior,
    },
    queue_bridge::{
        QueueEntry, QueueLane, QueueMirror, QueueOrigin, queue_changed_params, string_list,
    },
    recap_bridge::{parse_recap_message, session_recap_notification},
    subagent_projection::{BridgeOperation, bridge_parent_session_id, parse_bridge_message},
    todo_bridge::plan_update_for_tool,
    tool_projection::{
        bash_tool_output, edit_diff_content, history_tool_content, normalize_tool_raw_input,
        normalize_tool_raw_output, pi_result_text, tool_content, tool_kind,
    },
    workflow_host::{
        WorkflowHost, WorkflowRequest, format_outcome_for_tool, outcome_to_json,
        parse_workflow_request,
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
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
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

const EXTENSION_QUEUE_STATUS_KEY: &str = "__pi_grok_queue_enqueue__";

fn stop_reason_wire(reason: &acp::StopReason) -> &'static str {
    match reason {
        acp::StopReason::Cancelled => "cancelled",
        _ => "end_turn",
    }
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
    queued_prompt_completions: HashMap<String, oneshot::Sender<PromptCompletion>>,
    next_prompt_id: u64,
    agent_running: bool,
    cancelling: bool,
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
    /// UTC ms when the current Pi agent turn began (`turnStartMs` on live ACP
    /// notifications). Mirrors stock Grok shell so the pager can pre-create
    /// Thinking… and drive turn timers / breathing animation.
    turn_start_ms: Option<i64>,
    /// UTC ms when the current LLM stream segment began (`streamStartMs`).
    /// Bumped on each assistant `message_start` / `turn_start` so stream
    /// boundaries match Grok shell semantics.
    stream_start_ms: Option<i64>,
    /// Client `promptId` of the in-flight primary turn (Pager-minted). Stamped
    /// on every live ACP `SessionNotification._meta.promptId` so the pager's
    /// prompt-id gate and turn chrome match stock Grok shell.
    live_prompt_id: Option<String>,
    /// Last full bash stdout (bytes) per toolCallId — used to emit
    /// `BashOutput.output_delta` on `tool_execution_update` so Execute cards
    /// stream instead of only jumping at tool end.
    bash_stream_output: HashMap<String, Vec<u8>>,
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
    /// In-flight `/btw` side questions keyed by requestId (extension custom message).
    pending_btw: HashMap<String, oneshot::Sender<Result<String, String>>>,
    /// A recap command must finish before another recap can be requested.
    recap_in_flight: bool,
    /// Plan mode lifecycle tracker. The adapter is the sole owner of plan mode
    /// state — Pi RPC has no mode concept, and the Pager only renders.
    plan_mode: crate::plan_mode::PiPlanTracker,
    /// Process-private control file consumed by the injected Pi plan gate.
    /// It is deliberately not session persistence; the adapter rewrites it
    /// from its authoritative tracker after every transition.
    plan_mode_control: Option<PathBuf>,
}

#[derive(Clone)]
pub struct PiAgent {
    rpc: PiRpc,
    client_tx: mpsc::UnboundedSender<AcpClientMessage>,
    state: Rc<RefCell<AdapterState>>,
    bash_control_meta: Option<PathBuf>,
    /// Process-unique JSON path written by `__pi_context_breakdown`.
    context_breakdown: Option<PathBuf>,
    /// Channel to execute workflow spawn/cancel bridge commands on the LocalSet.
    workflow_bridge_tx: BridgeCommandTx,
    workflow_bridge_rx: Rc<RefCell<Option<mpsc::UnboundedReceiver<BridgeCommandRequest>>>>,
    /// Lazy session-scoped upstream workflow host (xai-workflow + Pi spawn).
    workflow_host: Rc<RefCell<Option<std::sync::Arc<WorkflowHost>>>>,
    /// F2 pi_goal control file + GoalHost (None when feature off).
    goal_host: Rc<RefCell<Option<GoalHost>>>,
}

impl PiAgent {
    pub fn new(
        rpc: PiRpc,
        client_tx: mpsc::UnboundedSender<AcpClientMessage>,
        bootstrap: PiBootstrap,
        session_dir: PathBuf,
        bash_control_meta: Option<PathBuf>,
        context_breakdown: Option<PathBuf>,
        plan_mode_control: Option<PathBuf>,
        goal_control: Option<PathBuf>,
    ) -> Result<Self> {
        let acp_session_id = bootstrap.state.session_id.clone();
        let plan_file = plan_file_path(&bootstrap.state, &session_dir);
        let plan_mode = load_plan_tracker(&plan_file)?;
        let model_map = bootstrap
            .models
            .iter()
            .cloned()
            .map(|model| (model_key(&model), model))
            .collect();
        let (workflow_bridge_tx, workflow_bridge_rx) = mpsc::unbounded_channel();
        Ok(Self {
            rpc,
            client_tx,
            bash_control_meta,
            context_breakdown,
            workflow_bridge_tx,
            workflow_bridge_rx: Rc::new(RefCell::new(Some(workflow_bridge_rx))),
            workflow_host: Rc::new(RefCell::new(None)),
            goal_host: Rc::new(RefCell::new(goal_control.map(GoalHost::new))),
            state: Rc::new(RefCell::new(AdapterState {
                bootstrap,
                acp_session_id,
                model_map,
                active_prompts: Vec::new(),
                queued_prompt_completions: HashMap::new(),
                next_prompt_id: 1,
                agent_running: false,
                cancelling: false,
                bash_running: false,
                live_assistant: None,
                session_dir: session_dir.clone(),
                session_paths: HashMap::new(),
                tool_args: HashMap::new(),
                last_context_tokens: None,
                turn_start_ms: None,
                stream_start_ms: None,
                live_prompt_id: None,
                bash_stream_output: HashMap::new(),
                compaction_started_at: None,
                queue_mirror: QueueMirror::default(),
                subagent_bridge_sequences: HashMap::new(),
                pending_subagent_bridge: PendingSubagentBridge::default(),
                pending_btw: HashMap::new(),
                recap_in_flight: false,
                plan_mode,
                plan_mode_control,
            })),
        })
    }

    pub async fn run_events(self: Rc<Self>, mut events: mpsc::UnboundedReceiver<Value>) {
        if let Some(mut bridge_rx) = self.workflow_bridge_rx.borrow_mut().take() {
            let agent = self.clone();
            tokio::task::spawn_local(async move {
                while let Some(req) = bridge_rx.recv().await {
                    let result = agent
                        .run_bridge_command(&req.command, &req.args)
                        .await
                        .map_err(|error| error.to_string());
                    let _ = req.reply.send(result);
                }
            });
        }
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
    pub async fn publish_session_catalog(&self, cwd: PathBuf, all: bool, use_psm_index: bool) {
        let session_dir = {
            let state = self.state.borrow();
            catalog_session_dir(&state.bootstrap.state, &state.session_dir)
        };
        let psm_cwd = cwd.clone();
        let sessions = tokio::task::spawn_blocking(move || {
            if use_psm_index {
                if let Some(sessions) = crate::psm_session_catalog::load_catalog(&psm_cwd, all) {
                    return sessions;
                }
            }
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
                    "summary": session.name.as_deref().unwrap_or(&session.first_message),
                    "name": session.name,
                    "firstMessage": session.first_message,
                    "sessionPath": session.path,
                    "cwd": session.cwd,
                    "createdAt": session.created_at,
                    "updatedAt": session.modified_at,
                    "modelId": session.model_id,
                    "totalTokens": session.total_tokens,
                    "totalCost": session.total_cost,
                    "messageCount": session.message_count,
                    "parentSessionPath": session.parent_session_path,
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
        let (tree, _) = self.fetch_session_tree_with_editor_text(None).await?;
        Ok(tree)
    }

    async fn fetch_session_tree_with_editor_text(
        &self,
        entry_id: Option<&str>,
    ) -> Result<(PiSessionTree, Option<String>)> {
        let entry_id = entry_id.map(str::to_owned);
        let data = self.rpc.request(json!({ "type": "get_tree" })).await?;
        tokio::task::spawn_blocking(move || {
            crate::pi_rpc::with_large_stack(move || {
                let editor_text = entry_id
                    .as_deref()
                    .and_then(|entry_id| tree_entry_editor_text(&data, entry_id));
                let tree = parse_session_tree(&data);
                drop(data);
                (tree, editor_text)
            })
        })
        .await
        .map_err(|error| anyhow!("Pi get_tree worker failed: {error}"))
    }

    /// Run a read-only bridge command without entering `active_prompts`.
    ///
    /// Pi executes extension commands immediately even during streaming and
    /// acknowledges RPC `prompt` only after their handler finishes. Tracking
    /// this as a normal prompt would instead bind it to the current turn's
    /// `agent_settled`, delaying `/context` until the agent becomes idle.
    async fn run_immediate_bridge_command(
        &self,
        command: &str,
        args: &str,
    ) -> Result<(), acp::Error> {
        self.require_bridge_command(command)?;
        let message = bridge_command_message(command, args);
        self.rpc
            .request(json!({ "type": "prompt", "message": message }))
            .await
            .map_err(acp_internal)?;
        Ok(())
    }

    /// Run a stateful hidden bridge extension command (`/__pi_*`) and wait for
    /// the non-agent preflight probe to complete.
    async fn run_bridge_command(&self, command: &str, args: &str) -> Result<(), acp::Error> {
        self.require_bridge_command(command)?;
        let message = bridge_command_message(command, args);
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

    fn require_bridge_command(&self, command: &str) -> Result<(), acp::Error> {
        if bridge_command_is_registered(&self.state.borrow().bootstrap.commands, command) {
            return Ok(());
        }
        Err(acp::Error::method_not_found().data(format!(
            "Pi bridge command /{command} is unavailable because its extension is not loaded."
        )))
    }

    /// True when F2 `[ui].pi_workflows` is on (and/or parent env set by grok-pi).
    ///
    /// Prefer disk config: env alone is unreliable because `PI_GROK_WORKFLOWS`
    /// was historically only injected into the Pi *child* process, while the
    /// adapter runs in the *parent* (grok-pi) process.
    fn workflows_extension_enabled() -> bool {
        if let Ok(config) = xai_grok_shell::config::load_effective_config() {
            if config
                .get("ui")
                .and_then(|ui| ui.get("pi_workflows"))
                .and_then(|v| v.as_bool())
                == Some(true)
            {
                return true;
            }
        }
        match std::env::var("PI_GROK_WORKFLOWS") {
            Ok(v) => {
                let v = v.trim();
                v == "1" || v.eq_ignore_ascii_case("true") || v.eq_ignore_ascii_case("on")
            }
            Err(_) => false,
        }
    }

    fn ensure_workflow_host(&self) -> Result<()> {
        if !Self::workflows_extension_enabled() {
            bail!(
                "Pi workflows is off. F2 → Agent → Pi workflows → on, fully quit, then restart grok-pi (extension injects only at startup)."
            );
        }
        if self.workflow_host.borrow().is_some() {
            return Ok(());
        }
        let (session_id, cwd, session_dir) = {
            let state = self.state.borrow();
            let session_id = state.acp_session_id.clone();
            let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            let session_dir = state
                .bootstrap
                .state
                .session_file
                .as_ref()
                .and_then(|p| Path::new(p).parent().map(|p| p.to_path_buf()))
                .or_else(|| Some(state.session_dir.clone()));
            (session_id, cwd, session_dir)
        };
        let host = std::sync::Arc::new(WorkflowHost::new(
            session_id,
            cwd,
            session_dir,
            self.workflow_bridge_tx.clone(),
        ));
        *self.workflow_host.borrow_mut() = Some(host);
        Ok(())
    }

    fn workflow_host_arc(&self) -> Result<std::sync::Arc<WorkflowHost>> {
        self.ensure_workflow_host()?;
        self.workflow_host
            .borrow()
            .clone()
            .ok_or_else(|| anyhow!("workflow host missing after ensure"))
    }

    async fn emit_workflow_notifications(&self) {
        let payloads = match self.workflow_host.borrow().as_ref() {
            Some(host) => host.notification_payloads(),
            None => Vec::new(),
        };
        for payload in payloads {
            self.send_ext_notification("x.ai/session_notification", payload)
                .await;
        }
    }

    async fn handle_workflow_request(&self, name: &str, args: &str) -> Result<Value, acp::Error> {
        let host = self.workflow_host_arc().map_err(acp_internal)?;
        let request = parse_workflow_request(name, args)
            .map_err(|e| acp::Error::invalid_params().data(e.to_string()))?;
        match request {
            WorkflowRequest::Launch {
                name,
                objective,
                args,
            } => {
                let (run_id, outcome_rx) = host
                    .launch_named(&name, objective, args)
                    .await
                    .map_err(acp_internal)?;
                self.emit_workflow_notifications().await;
                let agent = self.clone();
                let host_bg = host.clone();
                let run_id_ret = run_id.clone();
                // Fire-and-forget when called without a response file (ACP methods).
                // The tool path waits via `run_workflow_tool_to_completion`.
                tokio::task::spawn_local(async move {
                    let _ = host_bg
                        .drive_until_outcome(outcome_rx, |payload| {
                            let agent = agent.clone();
                            tokio::task::spawn_local(async move {
                                agent
                                    .send_ext_notification("x.ai/session_notification", payload)
                                    .await;
                            });
                        })
                        .await;
                    agent.emit_workflow_notifications().await;
                });
                Ok(json!({ "runId": run_id_ret, "started": true }))
            }
            WorkflowRequest::Manage { op, target } => {
                match op.as_str() {
                    "pause" => {
                        let ok = host.pause(&target).await;
                        self.emit_workflow_notifications().await;
                        Ok(json!({ "op": "pause", "target": target, "ok": ok }))
                    }
                    "stop" => {
                        let ok = host.cancel(&target).await;
                        self.emit_workflow_notifications().await;
                        Ok(json!({ "op": "stop", "target": target, "ok": ok }))
                    }
                    other => Err(acp::Error::invalid_params()
                        .data(format!("unsupported workflow op: {other}"))),
                }
            }
        }
    }

    /// Launch (or manage) and block until terminal outcome — used by the Pi
    /// `workflow` tool so the parent turn receives the real report text.
    async fn run_workflow_tool_to_completion(
        &self,
        name: &str,
        args: &str,
    ) -> Result<Value, acp::Error> {
        let host = self.workflow_host_arc().map_err(acp_internal)?;
        let request = parse_workflow_request(name, args)
            .map_err(|e| acp::Error::invalid_params().data(e.to_string()))?;
        match request {
            WorkflowRequest::Manage { op, target } => {
                match op.as_str() {
                    "pause" => {
                        let ok = host.pause(&target).await;
                        self.emit_workflow_notifications().await;
                        Ok(json!({ "op": "pause", "target": target, "ok": ok }))
                    }
                    "stop" => {
                        let ok = host.cancel(&target).await;
                        self.emit_workflow_notifications().await;
                        Ok(json!({ "op": "stop", "target": target, "ok": ok }))
                    }
                    other => Err(acp::Error::invalid_params()
                        .data(format!("unsupported workflow op: {other}"))),
                }
            }
            WorkflowRequest::Launch {
                name,
                objective,
                args,
            } => {
                let (run_id, outcome_rx) = host
                    .launch_named(&name, objective, args)
                    .await
                    .map_err(acp_internal)?;
                self.emit_workflow_notifications().await;
                let agent = self.clone();
                let host_bg = host.clone();
                let outcome = host_bg
                    .drive_until_outcome(outcome_rx, |payload| {
                        let agent = agent.clone();
                        tokio::task::spawn_local(async move {
                            agent
                                .send_ext_notification("x.ai/session_notification", payload)
                                .await;
                        });
                    })
                    .await
                    .map_err(acp_internal)?;
                agent.emit_workflow_notifications().await;
                let text = format_outcome_for_tool(&run_id, &outcome);
                Ok(json!({
                    "runId": run_id,
                    "outcome": outcome_to_json(&outcome),
                    "text": text,
                }))
            }
        }
    }

    async fn workflow_pause(&self, run_id: &str) -> Result<bool, acp::Error> {
        let host = self.workflow_host_arc().map_err(acp_internal)?;
        let ok = host.pause(run_id).await;
        self.emit_workflow_notifications().await;
        Ok(ok)
    }

    async fn workflow_cancel(&self, run_id: &str) -> Result<bool, acp::Error> {
        let host = self.workflow_host_arc().map_err(acp_internal)?;
        let ok = host.cancel(run_id).await;
        self.emit_workflow_notifications().await;
        Ok(ok)
    }

    async fn emit_goal_updated_from_control(&self, control: &GoalControl) {
        let session_id = self.session_id().0.to_string();
        let payload = {
            let host = self.goal_host.borrow();
            let Some(host) = host.as_ref() else {
                return;
            };
            host.notification_payload(&session_id, control)
        };
        self.send_ext_notification("x.ai/session_notification", payload)
            .await;
    }

    async fn refresh_goal_from_disk(&self) -> Option<GoalControl> {
        let mut host = self.goal_host.borrow_mut();
        let host = host.as_mut()?;
        host.load()
    }

    /// Extension bridge: control file already written; reload + GoalUpdated.
    async fn handle_goal_bridge_message(&self, event: &Value) -> Result<bool> {
        if self.goal_host.borrow().is_none() {
            return Ok(false);
        }
        let message = event
            .get("message")
            .or_else(|| event.get("entry"))
            .unwrap_or(event);
        let custom_type = message
            .get("customType")
            .and_then(Value::as_str)
            .or_else(|| message.get("type").and_then(Value::as_str));
        if custom_type != Some("pi-grok-goal/v1") {
            return Ok(false);
        }
        if let Some(control) = self.refresh_goal_from_disk().await {
            self.emit_goal_updated_from_control(&control).await;
        } else if let Some(control) = message
            .get("details")
            .or_else(|| message.get("data"))
            .and_then(|d| d.get("control"))
            .and_then(|c| serde_json::from_value::<GoalControl>(c.clone()).ok())
        {
            if let Some(host) = self.goal_host.borrow_mut().as_mut() {
                host.apply_control(control.clone());
            }
            self.emit_goal_updated_from_control(&control).await;
        }
        Ok(true)
    }

    /// Extension bridge: scheduled task created/fired/deleted → native tasks pane.
    async fn handle_loop_bridge_message(&self, event: &Value) -> Result<bool> {
        let message = event
            .get("message")
            .or_else(|| event.get("entry"))
            .unwrap_or(event);
        let custom_type = message
            .get("customType")
            .and_then(Value::as_str)
            .or_else(|| message.get("type").and_then(Value::as_str));
        if custom_type != Some("pi-grok-loop/v1") {
            return Ok(false);
        }
        let details = message
            .get("details")
            .or_else(|| message.get("data"))
            .cloned()
            .unwrap_or(Value::Null);
        let Some((event_name, task)) = loop_host::parse_loop_bridge(&details) else {
            return Ok(true);
        };
        let session_id = self.session_id().0.to_string();
        if let Some((method, payload)) =
            loop_host::scheduled_task_notification(&session_id, &event_name, &task)
        {
            self.send_ext_notification(&method, payload).await;
        }
        Ok(true)
    }

    /// On idle: if goal Active, inject follow-up continuation (legacy path).
    async fn maybe_continue_goal(&self) {
        if self.goal_host.borrow().is_none() {
            return;
        }
        // Avoid stacking continuations when the queue already has work.
        {
            let snap = self.state.borrow().queue_mirror.snapshot();
            if snap.follow_up_count > 0
                || snap.steering_count > 0
                || snap.running_prompt_id.is_some()
            {
                return;
            }
        }
        let control = match self.refresh_goal_from_disk().await {
            Some(c) if c.is_active() => c,
            Some(c) => {
                self.emit_goal_updated_from_control(&c).await;
                return;
            }
            None => return,
        };
        self.emit_goal_updated_from_control(&control).await;
        let directive = GoalHost::continuation_directive(&control);
        self.enqueue_extension_message(directive, Vec::new(), Some("followUp"))
            .await;
    }

    async fn handle_workflow_bridge_message(&self, event: &Value) -> Result<bool> {
        let message = event
            .get("message")
            .or_else(|| event.get("entry"))
            .unwrap_or(event);
        let custom_type = message
            .get("customType")
            .and_then(Value::as_str)
            .or_else(|| message.get("type").and_then(Value::as_str));
        if custom_type != Some("pi-grok-workflow/v1") {
            return Ok(false);
        }
        let details = message
            .get("details")
            .or_else(|| message.get("data"))
            .cloned()
            .unwrap_or(Value::Null);
        let kind = details.get("kind").and_then(Value::as_str).unwrap_or("");
        if kind == "tool_request" {
            let name = details
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let args = details
                .get("args")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let response_path = details
                .get("responsePath")
                .or_else(|| details.get("response_path"))
                .and_then(Value::as_str)
                .map(std::path::PathBuf::from);
            let agent = self.clone();
            // Never block the RPC event loop for long Rhai runs: write the
            // tool response file from a local task; the extension polls it.
            tokio::task::spawn_local(async move {
                let result = if response_path.is_some() {
                    agent.run_workflow_tool_to_completion(&name, &args).await
                } else {
                    agent.handle_workflow_request(&name, &args).await
                };
                match (result, response_path) {
                    (Ok(value), Some(path)) => {
                        if let Err(error) = std::fs::write(
                            &path,
                            serde_json::to_vec_pretty(&value).unwrap_or_default(),
                        ) {
                            tracing::warn!(%error, path = %path.display(), "workflow tool response write failed");
                        }
                    }
                    (Err(error), Some(path)) => {
                        let payload = json!({
                            "error": error.to_string(),
                            "text": format!("Workflow request failed: {error}"),
                        });
                        let _ = std::fs::write(
                            &path,
                            serde_json::to_vec_pretty(&payload).unwrap_or_default(),
                        );
                        agent
                            .send_ui_notification(
                                &format!("Workflow request failed: {error}"),
                                Some("error"),
                            )
                            .await;
                    }
                    (Err(error), None) => {
                        agent
                            .send_ui_notification(
                                &format!("Workflow request failed: {error}"),
                                Some("error"),
                            )
                            .await;
                    }
                    (Ok(_), None) => {}
                }
            });
            return Ok(true);
        }
        self.emit_workflow_notifications().await;
        Ok(true)
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
        self.run_bridge_command(NAVIGATE_TREE_COMMAND, &args)
            .await?;

        // Leaf moved inside the same session file. Refresh adapter state only;
        // the pager issues session/load to clear scrollback and re-replay.
        let bootstrap = self.refresh().await.map_err(acp_internal)?;
        let (tree, editor_text) = self
            .fetch_session_tree_with_editor_text(Some(entry_id))
            .await
            .map_err(acp_internal)?;
        Ok(json!({
            "sessionId": bootstrap.state.session_id,
            "leafId": tree.leaf_id,
            "editorText": editor_text,
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

    /// Read-only list of user messages available for Pi `/fork`.
    async fn fetch_fork_messages(&self) -> Result<Value, acp::Error> {
        let data = self
            .rpc
            .request(json!({ "type": "get_fork_messages" }))
            .await
            .map_err(acp_internal)?;
        let messages = data.get("messages").cloned().unwrap_or_else(|| json!([]));
        Ok(json!({ "messages": messages }))
    }

    /// Fork Pi into a new session file from a user-message entry.
    ///
    /// On success the adapter rebinds to the new session identity (same process,
    /// new JSONL) so subsequent `session/load` replays the forked history.
    async fn fork_from_entry(&self, entry_id: &str) -> Result<Value, acp::Error> {
        let entry_id = entry_id.trim();
        if entry_id.is_empty() {
            return Err(acp::Error::invalid_params().data("entryId is empty"));
        }
        let response = self
            .rpc
            .request(json!({
                "type": "fork",
                "entryId": entry_id,
            }))
            .await
            .map_err(acp_internal)?;
        let cancelled = response
            .get("cancelled")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let text = response
            .get("text")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if cancelled {
            return Ok(json!({
                "cancelled": true,
                "entryId": entry_id,
            }));
        }
        let bootstrap = self.rebind_after_session_branch().await?;
        Ok(json!({
            "cancelled": false,
            "entryId": entry_id,
            "sessionId": bootstrap.state.session_id,
            "sessionFile": bootstrap.state.session_file,
            "text": text,
        }))
    }

    /// Reload Pi settings, extensions, skills, prompts, themes, and context files.
    ///
    /// Uses injected `__pi_reload` → official `ctx.reload()` (RPC has no bare
    /// `reload` command). Refreshes adapter bootstrap so command/model catalogs
    /// match the reloaded runtime.
    async fn reload_session_resources(&self) -> Result<Value, acp::Error> {
        let state = parse_state(
            &self
                .rpc
                .request(json!({ "type": "get_state" }))
                .await
                .map_err(acp_internal)?,
        );
        if state.is_streaming {
            return Err(acp::Error::internal_error()
                .data("Wait for the current response to finish before reloading."));
        }
        if state.is_compacting {
            return Err(acp::Error::internal_error()
                .data("Wait for compaction to finish before reloading."));
        }
        self.run_bridge_command(RELOAD_COMMAND, "").await?;
        let bootstrap = self.refresh().await.map_err(acp_internal)?;
        self.publish_bootstrap(&bootstrap).await;
        Ok(json!({
            "ok": true,
            "sessionId": bootstrap.state.session_id,
        }))
    }

    /// Duplicate the current Pi leaf into a new session file (`position: "at"`).
    async fn clone_current_session(&self) -> Result<Value, acp::Error> {
        let response = self
            .rpc
            .request(json!({ "type": "clone" }))
            .await
            .map_err(acp_internal)?;
        let cancelled = response
            .get("cancelled")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if cancelled {
            return Ok(json!({ "cancelled": true }));
        }
        let bootstrap = self.rebind_after_session_branch().await?;
        Ok(json!({
            "cancelled": false,
            "sessionId": bootstrap.state.session_id,
            "sessionFile": bootstrap.state.session_file,
        }))
    }

    /// After Pi fork/clone replaces the runtime session file, rebind adapter state.
    async fn rebind_after_session_branch(&self) -> Result<PiBootstrap, acp::Error> {
        let bootstrap = self.refresh().await.map_err(acp_internal)?;
        if let Some(path) = bootstrap
            .state
            .session_file
            .as_deref()
            .filter(|path| !path.is_empty())
        {
            self.state
                .borrow_mut()
                .session_paths
                .insert(bootstrap.state.session_id.clone(), PathBuf::from(path));
        }
        {
            let mut state = self.state.borrow_mut();
            let plan_path = plan_file_path(&bootstrap.state, &state.session_dir);
            state.plan_mode = load_plan_tracker(&plan_path).map_err(acp_internal)?;
        }
        self.publish_bootstrap(&bootstrap).await;
        // Fork/clone rebinds session identity; refresh bar from the new JSONL.
        self.refresh_context_usage().await;
        Ok(bootstrap)
    }

    fn replace_bootstrap(&self, bootstrap: PiBootstrap) {
        let mut state = self.state.borrow_mut();
        let session_changed = state.acp_session_id != bootstrap.state.session_id;
        state.acp_session_id = bootstrap.state.session_id.clone();
        state.model_map = bootstrap
            .models
            .iter()
            .cloned()
            .map(|model| (model_key(&model), model))
            .collect();
        // A session change invalidates the previous session's isolated queue.
        // Dropping completion senders resolves waiting ACP requests as cancelled.
        state.queue_mirror = QueueMirror::default();
        state.queued_prompt_completions.clear();
        state.agent_running = false;
        state.cancelling = false;
        // Drop cached context usage so publish_bootstrap cannot re-stamp the
        // previous session's totalTokens onto a fresh AgentView (context bar).
        if session_changed {
            state.last_context_tokens = None;
            state.turn_start_ms = None;
            state.stream_start_ms = None;
            state.live_prompt_id = None;
            state.bash_stream_output.clear();
        }
        state.bootstrap = bootstrap;
    }

    fn session_id(&self) -> acp::SessionId {
        acp::SessionId::new(self.state.borrow().acp_session_id.clone())
    }

    /// ACP session modes advertised on new/load session responses.
    ///
    /// Pager plan-mode UI is driven by `modes` + `CurrentModeUpdate`, not by
    /// initialize-time agent capabilities. Mirror Grok's default/plan pair so
    /// Shift+Tab / F2 plan toggle can reach the adapter.
    fn acp_session_modes(&self) -> acp::SessionModeState {
        let current = {
            let state = self.state.borrow();
            match state.plan_mode.state() {
                crate::plan_mode::PiPlanState::Pending | crate::plan_mode::PiPlanState::Active => {
                    "plan"
                }
                crate::plan_mode::PiPlanState::ExitPending
                | crate::plan_mode::PiPlanState::Inactive => "default",
            }
        };
        acp::SessionModeState::new(
            acp::SessionModeId::new(current),
            vec![
                acp::SessionMode::new(acp::SessionModeId::new("default"), "Agent"),
                acp::SessionMode::new(acp::SessionModeId::new("plan"), "Plan Mode"),
            ],
        )
    }

    /// Atomically publish the plan gate inputs to the injected Pi extension.
    ///
    /// The adapter is the sole writer, and this method has no await point, so
    /// no two adapter tasks can interleave writes. Rename makes readers observe
    /// either the prior complete JSON document or the next complete document.
    fn sync_plan_mode_control(&self) -> Result<()> {
        let (control_path, active, plan_file_path) = {
            let state = self.state.borrow();
            let Some(control_path) = state.plan_mode_control.clone() else {
                return Ok(());
            };
            (
                control_path,
                state.plan_mode.is_active(),
                state.plan_mode.plan_file_path().display().to_string(),
            )
        };
        let body = serde_json::to_vec(&json!({
            "active": active,
            "planFilePath": plan_file_path,
        }))?;
        atomic_write(&control_path, &body)
    }

    /// Notify the Pager of the session plan file path so `/view-plan` and the
    /// plan preview overlay can locate the Pi-owned sidecar.
    async fn publish_plan_file_path(&self) {
        let plan_path = self
            .state
            .borrow()
            .plan_mode
            .plan_file_path()
            .display()
            .to_string();
        self.send_ext_notification("pi/ui/plan_file", json!({ "planFilePath": plan_path }))
            .await;
    }

    /// Persist the tracker after every durable state transition. The data is
    /// private to the Pi session sidecar; the Pi core remains unaware of it.
    fn persist_plan_mode_state(&self) -> Result<()> {
        let (path, snapshot) = {
            let state = self.state.borrow();
            (
                plan_state_path(state.plan_mode.plan_file_path()),
                state.plan_mode.snapshot(),
            )
        };
        let body = serde_json::to_vec(&snapshot)?;
        atomic_write(&path, &body)
    }

    async fn send_update(&self, update: acp::SessionUpdate) {
        let mut notification = acp::SessionNotification::new(self.session_id(), update);
        // Stamp the same live timing fields stock Grok shell puts on every
        // SessionNotification. Without streamStartMs the pager never pre-creates
        // Thinking… and the first-token empty window has no breathing accent.
        let mut meta = acp::Meta::new();
        {
            let state = self.state.borrow();
            if let Some(tokens) = state.last_context_tokens {
                meta.insert("totalTokens".into(), json!(tokens));
            }
            if let Some(ms) = state.turn_start_ms {
                meta.insert("turnStartMs".into(), json!(ms));
            }
            if let Some(ms) = state.stream_start_ms {
                meta.insert("streamStartMs".into(), json!(ms));
            }
            // Same pin stock Grok shell puts on every live notification.
            // Without promptId the pager can still render when current_prompt_id
            // is set locally, but turn-status / gate adoption stay incomplete.
            if let Some(pid) = state.live_prompt_id.as_ref() {
                meta.insert("promptId".into(), json!(pid));
            }
        }
        meta.insert("agentTimestampMs".into(), json!(utc_now_ms()));
        notification = notification.meta(Some(meta));
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
        let mut notification =
            acp::SessionNotification::new(acp::SessionId::new(session_id.to_string()), update);
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

    fn accept_subagent_bridge_sequence(
        &self,
        subagent_id: &str,
        sequence: u64,
        replay: bool,
    ) -> bool {
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

    fn handle_btw_bridge_message(&self, event: &Value) -> bool {
        let Some(projection) = parse_btw_message(event) else {
            return false;
        };
        let sender = self
            .state
            .borrow_mut()
            .pending_btw
            .remove(&projection.request_id);
        if let Some(tx) = sender {
            let _ = tx.send(projection.result);
        } else {
            tracing::debug!(
                request_id = %projection.request_id,
                "btw bridge message with no pending waiter"
            );
        }
        true
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
    /// system/tool estimates) but returns the ACP envelope that the pager's
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
            Ok(value)
                if value
                    .get("messages")
                    .and_then(Value::as_array)
                    .is_some_and(|m| !m.is_empty())
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
        let breakdown = self.fetch_context_breakdown().await;
        // Best-effort raw entries for cache graph (pi-cache-graph alignment).
        let entries_for_cache = self
            .rpc
            .request(json!({ "type": "get_entries" }))
            .await
            .ok();
        let (session_id, model, cached_tokens, session_file) = {
            let state = self.state.borrow();
            (
                state.acp_session_id.clone(),
                state.bootstrap.state.model.clone(),
                state.last_context_tokens,
                state.bootstrap.state.session_file.clone(),
            )
        };
        let cwd = std::env::current_dir()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut response = build_session_info_response(
            &stats,
            messages.as_ref(),
            &session_id,
            &cwd,
            model.as_ref(),
            cached_tokens,
            breakdown.as_ref(),
            session_file.as_deref(),
        );
        if let Some(entries) = entries_for_cache.as_ref() {
            let metrics = crate::cache_metrics::collect_cache_session_metrics(entries);
            response = crate::context_projection::attach_cache_metrics(response, metrics);
        }
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

    /// Best-effort system/tool/agents breakdown via the injected bridge extension.
    ///
    /// Failure is non-fatal: projection falls back to system/tools = 0.
    async fn fetch_context_breakdown(
        &self,
    ) -> Option<crate::context_projection::ContextBreakdownRaw> {
        let path = self.context_breakdown.as_ref()?;
        if let Err(error) = self
            .run_immediate_bridge_command(CONTEXT_BREAKDOWN_COMMAND, "")
            .await
        {
            tracing::debug!(?error, "context breakdown bridge failed");
            return None;
        }
        let bytes = match std::fs::read(path) {
            Ok(bytes) if !bytes.is_empty() => bytes,
            Ok(_) => return None,
            Err(error) => {
                tracing::debug!(?error, path = %path.display(), "context breakdown file missing");
                return None;
            }
        };
        match serde_json::from_slice::<Value>(&bytes) {
            Ok(value) => Some(parse_context_breakdown(&value)),
            Err(error) => {
                tracing::debug!(?error, "context breakdown JSON invalid");
                None
            }
        }
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
            (state.acp_session_id.clone(), state.queue_mirror.snapshot())
        };
        let steering_text =
            (snapshot.steering_count > 0).then(|| format!("{} steering", snapshot.steering_count));
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
            state.queue_mirror.apply_queue_update(&steering, &follow_up);
        }
        self.publish_queue_snapshot().await;
    }

    async fn rebroadcast_queue_mirror(&self) {
        self.publish_queue_snapshot().await;
    }

    fn adapter_busy(state: &AdapterState) -> bool {
        state.agent_running
            || state.cancelling
            || state.bash_running
            || !state.active_prompts.is_empty()
            || state.queue_mirror.running().is_some()
    }

    fn prepare_execution_text(state: &mut AdapterState, mut message: String) -> String {
        if let Some(reminder) = state.plan_mode.build_reminder_for_prompt() {
            if message.is_empty() {
                message = reminder;
            } else {
                message = format!("{reminder}\n\n{message}");
            }
        }
        message
    }

    fn finish_queued_entries(&self, entries: Vec<QueueEntry>, reason: acp::StopReason) {
        let completions = {
            let mut state = self.state.borrow_mut();
            entries
                .into_iter()
                .filter_map(|entry| {
                    state
                        .queued_prompt_completions
                        .remove(&entry.id)
                        .map(|completion| (entry.id, completion))
                })
                .collect::<Vec<_>>()
        };
        for (id, completion) in completions {
            let _ = completion.send(PromptCompletion {
                reason: reason.clone(),
                client_prompt_id: Some(id),
            });
        }
    }

    async fn send_server_prompt_complete(&self, entry: &QueueEntry, reason: acp::StopReason) {
        self.send_ext_notification(
            "x.ai/session/prompt_complete",
            json!({
                "sessionId": self.session_id().0,
                "promptId": entry.id,
                "stopReason": stop_reason_wire(&reason),
                "agentResult": Value::Null,
            }),
        )
        .await;
    }

    fn activate_queued_client(state: &mut AdapterState, entry_id: &str) -> Option<u64> {
        let completion = state.queued_prompt_completions.remove(entry_id)?;
        let id = state.next_prompt_id;
        state.next_prompt_id = state.next_prompt_id.wrapping_add(1).max(1);
        state.active_prompts.push(ActivePrompt {
            id,
            client_prompt_id: Some(entry_id.to_string()),
            completion,
            agent_started: false,
            cancelled: false,
        });
        Some(id)
    }

    fn take_active_prompt(&self, id: u64) -> Option<ActivePrompt> {
        let mut state = self.state.borrow_mut();
        let index = state
            .active_prompts
            .iter()
            .position(|active| active.id == id)?;
        Some(state.active_prompts.remove(index))
    }

    async fn dispatch_next_queued(&self) -> bool {
        loop {
            let Some((entry, active_id)) = ({
                let mut state = self.state.borrow_mut();
                if Self::adapter_busy(&state) {
                    None
                } else {
                    state.queue_mirror.pop_next_local().map(|mut entry| {
                        entry.execution_text =
                            Self::prepare_execution_text(&mut state, entry.execution_text);
                        let active_id = Self::activate_queued_client(&mut state, &entry.id);
                        state.agent_running = true;
                        state.live_prompt_id = Some(entry.id.clone());
                        state.queue_mirror.set_running(entry.clone());
                        (entry, active_id)
                    })
                }
            }) else {
                return false;
            };

            if let Err(error) = self.persist_plan_mode_state() {
                tracing::debug!(%error, "failed to persist plan mode before queued dispatch");
            }
            if let Err(error) = self.sync_plan_mode_control() {
                tracing::debug!(%error, "failed to sync plan mode before queued dispatch");
            }
            self.publish_queue_snapshot().await;

            let mut request = json!({
                "type": "prompt",
                "message": entry.execution_text,
            });
            if !entry.images.is_empty() {
                request["images"] = Value::Array(entry.images.clone());
            }
            match self.rpc.request(request).await {
                Ok(_) => {
                    // Extension-owned rows have no ACP completion waiter, but
                    // they still need the same idle probe. Another input
                    // handler may consume the promoted RPC prompt without
                    // producing agent_start/agent_settled; without this probe
                    // the native queue would remain pinned to a ghost run.
                    let probe = self.clone();
                    tokio::task::spawn_local(async move {
                        probe.probe_prompt_without_agent().await;
                    });
                    return true;
                }
                Err(error) => {
                    tracing::warn!(%error, prompt_id = %entry.id, "queued Pi prompt failed");
                    let active = active_id.and_then(|id| self.take_active_prompt(id));
                    {
                        let mut state = self.state.borrow_mut();
                        state.agent_running = false;
                        state.live_prompt_id = None;
                        state.queue_mirror.clear_running();
                    }
                    if let Some(active) = active {
                        let _ = active.completion.send(PromptCompletion {
                            reason: acp::StopReason::Cancelled,
                            client_prompt_id: active.client_prompt_id,
                        });
                    } else {
                        self.send_server_prompt_complete(&entry, acp::StopReason::Cancelled)
                            .await;
                    }
                    self.send_ui_notification(
                        &format!("Queued Pi message failed: {error}"),
                        Some("error"),
                    )
                    .await;
                    self.publish_queue_snapshot().await;
                }
            }
        }
    }

    async fn enqueue_extension_message(
        &self,
        text: String,
        images: Vec<Value>,
        streaming_behavior: Option<&str>,
    ) {
        if text.trim().is_empty() && images.is_empty() {
            return;
        }
        let lane = if streaming_behavior == Some("steer") {
            QueueLane::Steering
        } else {
            QueueLane::FollowUp
        };
        let (id, version, should_interject) = {
            let mut state = self.state.borrow_mut();
            // Cancel is a hard barrier for extension continuations. In
            // particular, loop.ts may call sendUserMessage() from its terminal
            // handler while Pi is aborting; accepting that follow-up here
            // would restart the loop as soon as cancellation settles.
            if state.cancelling {
                tracing::debug!("dropping extension queue message during cancellation");
                return;
            }
            let id = state.queue_mirror.enqueue_local(
                None,
                text.clone(),
                text,
                images,
                lane,
                QueueOrigin::Extension,
            );
            let version = state
                .queue_mirror
                .local_entry(&id)
                .map(|entry| entry.version)
                .unwrap_or(0);
            let should_interject =
                lane == QueueLane::Steering && state.agent_running && !state.cancelling;
            (id, version, should_interject)
        };
        self.publish_queue_snapshot().await;
        if should_interject {
            self.interject_local_queue(&id, Some(version), None).await;
        } else {
            self.dispatch_next_queued().await;
        }
    }

    async fn interject_local_queue(
        &self,
        id: &str,
        expected_version: Option<u64>,
        new_text: Option<String>,
    ) -> bool {
        let mut dispatch_idle = false;
        let steer = {
            let mut state = self.state.borrow_mut();
            let Some(mut entry) = state.queue_mirror.take_local(id, expected_version) else {
                return false;
            };
            if let Some(text) = new_text {
                entry.execution_text = text.clone();
                entry.display_text = text;
                entry.version = entry.version.wrapping_add(1);
            }
            if state.cancelling {
                state.queue_mirror.push_front_local(entry);
                None
            } else if !state.agent_running && state.active_prompts.is_empty() {
                state.queue_mirror.push_front_local(entry);
                dispatch_idle = true;
                None
            } else {
                entry.execution_text =
                    Self::prepare_execution_text(&mut state, entry.execution_text);
                let active_id = Self::activate_queued_client(&mut state, &entry.id);
                state.queue_mirror.reserve(
                    entry.id.clone(),
                    entry.execution_text.clone(),
                    entry.display_text.clone(),
                    entry.images.clone(),
                    QueueLane::Steering,
                    entry.origin,
                );
                Some((entry, active_id))
            }
        };
        self.publish_queue_snapshot().await;
        if dispatch_idle {
            return self.dispatch_next_queued().await;
        }
        let Some((entry, active_id)) = steer else {
            return false;
        };
        let mut request = json!({
            "type": "prompt",
            "message": entry.execution_text,
            "streamingBehavior": "steer",
        });
        if !entry.images.is_empty() {
            request["images"] = Value::Array(entry.images.clone());
        }
        if let Err(error) = self.rpc.request(request).await {
            tracing::warn!(%error, prompt_id = %entry.id, "queued interject failed");
            self.state
                .borrow_mut()
                .queue_mirror
                .release_reservation(&entry.id);
            if let Some(active) = active_id.and_then(|id| self.take_active_prompt(id)) {
                let _ = active.completion.send(PromptCompletion {
                    reason: acp::StopReason::Cancelled,
                    client_prompt_id: active.client_prompt_id,
                });
            }
            self.send_ui_notification(
                &format!("Pi queue interject failed: {error}"),
                Some("error"),
            )
            .await;
            self.publish_queue_snapshot().await;
            return false;
        }
        true
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
        for entry in parse_messages(&data) {
            self.replay_history_item(entry).await;
        }
        Ok(())
    }

    async fn replay_history_item(&self, entry: PiReplayEntry) {
        let timestamp_ms = entry.timestamp_ms;
        let update = match entry.item {
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
        self.send_replay_update(update, timestamp_ms).await;
    }

    /// Send a session update during history replay, stamping the original
    /// message timestamp (`agentTimestampMs`) so the pager can display the real
    /// creation time instead of the resume wall-clock time.
    async fn send_replay_update(&self, update: acp::SessionUpdate, timestamp_ms: Option<i64>) {
        let mut notification = acp::SessionNotification::new(self.session_id(), update);
        let mut meta = acp::Meta::new();
        meta.insert("isReplay".into(), Value::Bool(true));
        if let Some(ms) = timestamp_ms {
            meta.insert("agentTimestampMs".into(), json!(ms));
        }
        if let Some(tokens) = self.state.borrow().last_context_tokens {
            meta.insert("totalTokens".into(), json!(tokens));
        }
        notification = notification.meta(Some(meta));
        if let Err(error) = acp_send(notification, &self.client_tx).await {
            tracing::debug!(%error, "Grok pager closed while sending Pi replay update");
        }
    }

    async fn handle_event(&self, event: Value) -> Result<()> {
        let event_type = event
            .get("type")
            .or_else(|| event.get("event"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let suppress_cancelled_stream = self.state.borrow().cancelling
            && matches!(
                event_type,
                "turn_start"
                    | "turn_end"
                    | "message_start"
                    | "message_update"
                    | "message_end"
                    | "tool_execution_start"
                    | "tool_execution_update"
                    | "tool_execution_end"
                    | "agent_end"
            );
        if suppress_cancelled_stream {
            return Ok(());
        }
        match event_type {
            "agent_start" => {
                let now = utc_now_ms();
                let mut state = self.state.borrow_mut();
                state.agent_running = true;
                for active in &mut state.active_prompts {
                    active.agent_started = true;
                }
                if !state.cancelling {
                    state.turn_start_ms = Some(now);
                    state.stream_start_ms = Some(now);
                    if state.live_prompt_id.is_none() {
                        state.live_prompt_id = state
                            .active_prompts
                            .iter()
                            .rev()
                            .find_map(|p| p.client_prompt_id.clone())
                            .or_else(|| state.queue_mirror.running().map(|entry| entry.id.clone()));
                    }
                }
            }
            "agent_settled" => {
                self.refresh_context_usage().await;
                let (mode_update, running) = {
                    let mut state = self.state.borrow_mut();
                    state.agent_running = false;
                    state.cancelling = false;
                    state.turn_start_ms = None;
                    state.stream_start_ms = None;
                    state.live_prompt_id = None;
                    state.bash_stream_output.clear();
                    let running = state.queue_mirror.clear_running();
                    let mode_update = if matches!(
                        state.plan_mode.state(),
                        crate::plan_mode::PiPlanState::ExitPending
                    ) {
                        state.plan_mode.complete_deferred_exit();
                        Some(acp::SessionModeId::new("default"))
                    } else {
                        None
                    };
                    (mode_update, running)
                };
                if let Some(mode_id) = mode_update {
                    self.persist_plan_mode_state()?;
                    self.sync_plan_mode_control()?;
                    self.send_update(acp::SessionUpdate::CurrentModeUpdate(
                        acp::CurrentModeUpdate::new(mode_id),
                    ))
                    .await;
                }
                self.rebroadcast_queue_mirror().await;
                self.finish_prompts(acp::StopReason::EndTurn);
                if let Some(entry) = running
                    && entry.origin != QueueOrigin::Client
                {
                    self.send_server_prompt_complete(&entry, acp::StopReason::EndTurn)
                        .await;
                }
                let dispatched = self.dispatch_next_queued().await;
                if !dispatched {
                    self.maybe_continue_goal().await;
                }
            }
            // `agent_end` is not the Pi idle barrier. Retry, compaction and
            // extension handlers can continue after it; `agent_settled` owns
            // ACP prompt completion.
            "agent_end" => {}
            "turn_start" => {
                // Multi-turn agent loops: each turn is a new stream segment.
                let now = utc_now_ms();
                let mut state = self.state.borrow_mut();
                if state.turn_start_ms.is_none() {
                    state.turn_start_ms = Some(now);
                }
                state.stream_start_ms = Some(now);
            }
            "turn_end" => self.refresh_context_usage().await,
            "message_start" => self.handle_message_start(&event),
            "message_update" => self.handle_message_update(&event).await,
            "message_end" => {
                if self.handle_btw_bridge_message(&event)
                    || self.handle_recap_bridge_message(&event).await?
                    || self.handle_background_bash_bridge_message(&event).await?
                    || self.handle_workflow_bridge_message(&event).await?
                    || self.handle_goal_bridge_message(&event).await?
                    || self.handle_loop_bridge_message(&event).await?
                {
                    // Bridge custom messages are display/control traffic.
                } else if !self.handle_subagent_bridge_message(&event).await? {
                    self.handle_message_end(&event).await;
                }
            }
            // Live subagent bridge traffic is persisted with appendEntry so it
            // cannot enter Pi's steering/follow-up queues while the parent is
            // streaming. RPC exposes that append as entry_appended.
            "entry_appended" => {
                if self.handle_btw_bridge_message(&event) {
                    // /btw answer may arrive via appendEntry when parent is streaming.
                } else if !self.handle_workflow_bridge_message(&event).await?
                    && !self.handle_goal_bridge_message(&event).await?
                    && !self.handle_loop_bridge_message(&event).await?
                {
                    self.handle_subagent_bridge_message(&event).await?;
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
                let (queued, running) = {
                    let mut state = self.state.borrow_mut();
                    state.agent_running = false;
                    state.cancelling = false;
                    let queued = state.queue_mirror.clear_local();
                    let running = state.queue_mirror.clear_running();
                    state.queue_mirror.clear();
                    (queued, running)
                };
                self.finish_queued_entries(queued, acp::StopReason::Cancelled);
                self.finish_prompts(acp::StopReason::Cancelled);
                if let Some(entry) = running
                    && entry.origin != QueueOrigin::Client
                {
                    self.send_server_prompt_complete(&entry, acp::StopReason::Cancelled)
                        .await;
                }
                self.publish_queue_snapshot().await;
                self.send_ui_notification(message, Some("error")).await;
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_message_start(&self, event: &Value) {
        if message_role(event) == Some("assistant") {
            let now = utc_now_ms();
            let mut state = self.state.borrow_mut();
            state.live_assistant = Some(StreamSeen::default());
            // New assistant message = new LLM stream segment (same semantics
            // as Grok shell StreamStarted → record_stream_start).
            if state.turn_start_ms.is_none() {
                state.turn_start_ms = Some(now);
            }
            state.stream_start_ms = Some(now);
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
        if let Some(tokens) = message.get("usage").and_then(context_tokens_from_usage) {
            self.note_context_tokens(tokens);
        }
        let terminal_error = string(message, &["errorMessage", "error_message"])
            .filter(|error| !error.is_empty())
            .map(ToOwned::to_owned);
        for entry in parse_messages(&json!({ "messages": [message] })) {
            match entry.item {
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
        let active_prompts = {
            let mut state = self.state.borrow_mut();
            // Drop stream anchors so idle notifications do not re-trigger
            // Pager Thinking pre-create with a stale streamStartMs.
            state.turn_start_ms = None;
            state.stream_start_ms = None;
            state.live_prompt_id = None;
            state.bash_stream_output.clear();
            std::mem::take(&mut state.active_prompts)
        };
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

    /// Poll until Pi is truly idle after a fire-and-forget abort. ACP prompt
    /// completion already happened synchronously in `cancel()`; this only
    /// re-opens the adapter scheduler once the underlying Pi run has stopped.
    async fn settle_cancelled_prompts(&self) {
        const SETTLE_POLL_INTERVAL: Duration = Duration::from_millis(100);
        const SETTLE_POLL_DEADLINE: Duration = Duration::from_secs(30);
        let deadline = Instant::now() + SETTLE_POLL_DEADLINE;
        loop {
            if !self.state.borrow().cancelling {
                return;
            }
            let Ok(value) = self.rpc.request(json!({ "type": "get_state" })).await else {
                return;
            };
            if !parse_state(&value).is_streaming {
                {
                    let mut state = self.state.borrow_mut();
                    state.agent_running = false;
                    state.cancelling = false;
                    state.live_prompt_id = None;
                    state.queue_mirror.clear_running();
                }
                self.publish_queue_snapshot().await;
                self.dispatch_next_queued().await;
                return;
            }
            if Instant::now() >= deadline {
                tracing::warn!("Pi still streaming after asynchronous cancel deadline");
                self.state.borrow_mut().cancelling = false;
                return;
            }
            tokio::time::sleep(SETTLE_POLL_INTERVAL).await;
        }
    }

    async fn probe_prompt_without_agent(&self) {
        tokio::time::sleep(Duration::from_millis(40)).await;
        let should_probe = {
            let state = self.state.borrow();
            state
                .active_prompts
                .iter()
                .any(|active| !active.agent_started)
                || (state.queue_mirror.running().is_some() && state.agent_running)
        };
        if !should_probe {
            return;
        }
        let Ok(value) = self.rpc.request(json!({ "type": "get_state" })).await else {
            return;
        };
        if parse_state(&value).is_streaming {
            return;
        }
        let running = {
            let mut state = self.state.borrow_mut();
            state.agent_running = false;
            state.cancelling = false;
            state.live_prompt_id = None;
            state.turn_start_ms = None;
            state.stream_start_ms = None;
            state.queue_mirror.clear_running()
        };
        self.publish_queue_snapshot().await;
        self.finish_prompts(acp::StopReason::EndTurn);
        if let Some(entry) = running
            && entry.origin != QueueOrigin::Client
        {
            self.send_server_prompt_complete(&entry, acp::StopReason::EndTurn)
                .await;
        }
        let dispatched = self.dispatch_next_queued().await;
        if !dispatched {
            self.maybe_continue_goal().await;
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
                let raw_output = bash_tool_output(&command, None, &result, failed && !cancelled);
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
        let ask_user_args = (name == "ask_user_question")
            .then(|| args.clone())
            .flatten();
        // When the tool is Q&A we still spawn even if args is None so the control
        // file gets an error payload instead of leaving the extension polling forever.
        let open_ask_user = name == "ask_user_question";
        self.send_update(acp::SessionUpdate::ToolCall(
            acp::ToolCall::new(acp::ToolCallId::new(id.to_string()), name.to_string())
                .kind(tool_kind(name))
                .status(acp::ToolCallStatus::InProgress)
                .content(content)
                .locations(Vec::new())
                .raw_input(args),
        ))
        .await;
        if name == "exit_plan_mode" {
            self.request_plan_approval(id).await;
        }
        if open_ask_user {
            let agent = self.clone();
            let tool_call_id = id.to_string();
            tokio::task::spawn_local(async move {
                agent
                    .request_ask_user_question(&tool_call_id, ask_user_args)
                    .await;
            });
        }
    }

    /// Bridge Pi extension `ask_user_question` to native Grok QuestionView.
    ///
    /// Writes `{outcome,message}` under `PI_GROK_ASK_USER_DIR/<toolCallId>.json`
    /// so the injected extension can complete its blocking execute.
    async fn request_ask_user_question(&self, tool_call_id: &str, args: Option<Value>) {
        let Some(dir) = std::env::var_os("PI_GROK_ASK_USER_DIR") else {
            write_ask_user_response(
                tool_call_id,
                json!({
                    "outcome": "error",
                    "message": "ask_user_question host control missing (enable F2 Q&A and restart).",
                }),
            );
            return;
        };
        let questions = normalize_ask_user_questions(args.as_ref());
        if questions.is_empty() {
            write_ask_user_response(
                tool_call_id,
                json!({
                    "outcome": "error",
                    "message": "ask_user_question requires at least one question with options.",
                }),
            );
            return;
        }
        let mode = if self.state.borrow().plan_mode.is_active() {
            "plan"
        } else {
            "default"
        };
        let params = json!({
            "sessionId": self.session_id().0.to_string(),
            "toolCallId": tool_call_id,
            "questions": questions,
            "mode": mode,
        });
        let raw = match serde_json::value::to_raw_value(&params) {
            Ok(raw) => raw,
            Err(error) => {
                write_ask_user_response(
                    tool_call_id,
                    json!({
                        "outcome": "error",
                        "message": format!("failed to serialize Q&A request: {error}"),
                    }),
                );
                return;
            }
        };
        let request = acp::ExtRequest::new("x.ai/ask_user_question", raw.into());
        let response = match acp_send(request, &self.client_tx).await {
            Ok(response) => response,
            Err(error) => {
                write_ask_user_response(
                    tool_call_id,
                    json!({
                        "outcome": "error",
                        "message": format!("Q&A request failed: {error}"),
                    }),
                );
                return;
            }
        };
        let outer: Value = match serde_json::from_str(response.0.get()) {
            Ok(value) => value,
            Err(error) => {
                write_ask_user_response(
                    tool_call_id,
                    json!({
                        "outcome": "error",
                        "message": format!("invalid Q&A response: {error}"),
                    }),
                );
                return;
            }
        };
        let result = outer.get("result").unwrap_or(&outer);
        let payload = format_ask_user_tool_result(result);
        let path = std::path::Path::new(&dir).join(format!("{tool_call_id}.json"));
        if let Err(error) = std::fs::write(&path, payload.to_string()) {
            tracing::warn!(%error, path = %path.display(), "failed to write ask_user_question response");
        }
    }

    /// Bridge Pi's extension-owned `exit_plan_mode` tool to the Pager's
    /// native PlanApprovalView. The adapter remains the state authority; the
    /// extension only gives the model a real tool to request this transition.
    async fn request_plan_approval(&self, tool_call_id: &str) {
        let plan_file_path = {
            let mut state = self.state.borrow_mut();
            if !state.plan_mode.is_active() || state.plan_mode.is_awaiting_plan_approval() {
                return;
            }
            state.plan_mode.set_awaiting_plan_approval(true);
            state.plan_mode.plan_file_path().to_path_buf()
        };
        if let Err(error) = self.persist_plan_mode_state() {
            tracing::warn!(%error, "failed to persist plan approval state");
            self.state
                .borrow_mut()
                .plan_mode
                .set_awaiting_plan_approval(false);
            return;
        }
        if let Err(error) = self.sync_plan_mode_control() {
            tracing::warn!(%error, "failed to publish plan gate before approval");
            self.state
                .borrow_mut()
                .plan_mode
                .set_awaiting_plan_approval(false);
            return;
        }
        let plan_content = std::fs::read_to_string(&plan_file_path)
            .ok()
            .filter(|content| !content.trim().is_empty());
        let params = json!({
            "sessionId": self.session_id().0.to_string(),
            "toolCallId": tool_call_id,
            "planContent": plan_content,
        });
        let raw = match serde_json::value::to_raw_value(&params) {
            Ok(raw) => raw,
            Err(error) => {
                tracing::warn!(%error, "failed to serialize plan approval request");
                self.state
                    .borrow_mut()
                    .plan_mode
                    .set_awaiting_plan_approval(false);
                return;
            }
        };
        let request = acp::ExtRequest::new("x.ai/exit_plan_mode", raw.into());
        let response = match acp_send(request, &self.client_tx).await {
            Ok(response) => response,
            Err(error) => {
                tracing::warn!(%error, "plan approval request failed");
                self.state
                    .borrow_mut()
                    .plan_mode
                    .set_awaiting_plan_approval(false);
                return;
            }
        };
        let response_value: Value = match serde_json::from_str(response.0.get()) {
            Ok(value) => value,
            Err(error) => {
                tracing::warn!(%error, "invalid plan approval response");
                self.state
                    .borrow_mut()
                    .plan_mode
                    .set_awaiting_plan_approval(false);
                return;
            }
        };
        let result = response_value.get("result").unwrap_or(&response_value);
        let outcome = result
            .get("outcome")
            .and_then(Value::as_str)
            .unwrap_or("cancelled");
        let feedback = result
            .get("feedback")
            .and_then(Value::as_str)
            .filter(|feedback| !feedback.trim().is_empty());
        let approved = outcome == "approved" || outcome == "abandoned";
        if approved {
            let changed = self.state.borrow_mut().plan_mode.deactivate_approved();
            if let Err(error) = self.persist_plan_mode_state() {
                tracing::warn!(%error, "failed to persist approved plan-mode exit");
            }
            if let Err(error) = self.sync_plan_mode_control() {
                tracing::warn!(%error, "failed to publish approved plan-mode exit");
            }
            if changed {
                self.send_update(acp::SessionUpdate::CurrentModeUpdate(
                    acp::CurrentModeUpdate::new(acp::SessionModeId::new("default")),
                ))
                .await;
            }
            return;
        }
        self.state
            .borrow_mut()
            .plan_mode
            .set_awaiting_plan_approval(false);
        if let Err(error) = self.persist_plan_mode_state() {
            tracing::warn!(%error, "failed to persist rejected plan approval");
        }
        if let Some(feedback) = feedback {
            let _ = self.rpc.notify(json!({
                "type": "follow_up",
                "message": format!("The user requested plan changes:\n{feedback}"),
            }));
        }
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
        // Execute streaming: stock Grok shell sends BashOutput with
        // `output_delta` so the pager appends and keeps is_running. Pi only
        // gives growing full text in partialResult; convert the growth into
        // output_delta so Run/bash cards actually breathe mid-command.
        let raw_output = if tool_kind(name) == acp::ToolKind::Execute {
            let command = args
                .as_ref()
                .and_then(|value| string(value, &["command", "cmd"]))
                .unwrap_or_default()
                .to_string();
            let description = args.as_ref().and_then(|value| {
                string(value, &["description", "task_name"])
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(ToOwned::to_owned)
            });
            let full_text = pi_result_text(&output);
            let full_bytes = full_text.as_bytes().to_vec();
            let prev = self
                .state
                .borrow_mut()
                .bash_stream_output
                .insert(id.to_string(), full_bytes.clone())
                .unwrap_or_default();
            let delta = if full_bytes.starts_with(&prev) {
                full_bytes[prev.len()..].to_vec()
            } else {
                // Truncation / reset: send full buffer as delta after empty
                // (tracker append path only; set_execute_output on full replace).
                full_bytes.clone()
            };
            json!({
                "type": "Bash",
                "output": full_bytes,
                "output_for_prompt": full_text,
                "exit_code": 0,
                "command": command,
                "truncated": false,
                "signal": null,
                "timed_out": false,
                "description": description,
                "current_dir": "",
                "output_file": "",
                "total_bytes": full_bytes.len(),
                "was_bare_echo": false,
                "output_delta": delta,
            })
        } else {
            normalize_tool_raw_output(name, args.as_ref(), &output, false)
        };
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
        self.state.borrow_mut().bash_stream_output.remove(id);
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
        // Goal control file is the SSOT; tool_end reloads if bridge entry lags.
        if name.eq_ignore_ascii_case("update_goal")
            && let Some(control) = self.refresh_goal_from_disk().await
        {
            self.emit_goal_updated_from_control(&control).await;
        }
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
                if key == EXTENSION_QUEUE_STATUS_KEY {
                    if let Some(payload) =
                        text.and_then(|text| serde_json::from_str::<Value>(text).ok())
                    {
                        let message = string(&payload, &["text"]).unwrap_or_default().to_string();
                        let images = payload
                            .get("images")
                            .and_then(Value::as_array)
                            .cloned()
                            .unwrap_or_default();
                        let behavior = string(&payload, &["streamingBehavior", "deliverAs"]);
                        self.enqueue_extension_message(message, images, behavior)
                            .await;
                    }
                } else {
                    self.send_status(key, text.filter(|text| !text.is_empty()))
                        .await;
                }
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
        let raw_title = string(&event, &["title", "message"]).unwrap_or("Pi extension");
        // grok-pi's subagent configuration uses Pi's standard `ui.select`
        // callback, but requests the existing native QuestionView multi-select
        // affordance through a namespaced title envelope. Pi core remains
        // unchanged and other extension selects keep their single-choice
        // semantics.
        let multi_select_title = (method == "select")
            .then(|| extension_multi_select_title(raw_title))
            .flatten();
        let resource_picker = (method == "select")
            .then(|| extension_resource_picker(raw_title))
            .flatten();
        let resource_picker_title = resource_picker
            .as_ref()
            .and_then(|picker| picker.get("title"))
            .and_then(Value::as_str);
        let title = resource_picker_title
            .or(multi_select_title.as_deref())
            .unwrap_or(raw_title);
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
        let mut params = json!({
            "sessionId": self.session_id().0.to_string(),
            "toolCallId": tool_call_id.clone(),
            "questions": [{
                "question": question,
                "options": options,
                "multiSelect": multi_select_title.is_some(),
                "id": "pi-question",
            }],
            "mode": "default",
            "initialText": initial_text,
            "noFreeform": method == "select" || method == "confirm",
        });
        if let Some(resource_picker) = resource_picker.clone() {
            params
                .as_object_mut()
                .expect("extension question params must be an object")
                .insert("piGrokResourcePicker".to_owned(), resource_picker);
        }
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
        let answer = if resource_picker.is_some() {
            extension_resource_picker_answer(result).unwrap_or_else(|| "[]".to_string())
        } else if multi_select_title.is_some() {
            extension_multi_select_answer(result).unwrap_or_else(|| "[]".to_string())
        } else {
            extension_answer(&method, result).unwrap_or_default()
        };
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
        // Only advertise recap when its injected bridge command is available.
        let meta = json!({ "sessionRecap": recap_extension_enabled() })
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
        // Fresh session starts outside plan mode and receives its own JSONL
        // sidecar plan file rather than sharing the configured session root.
        {
            let mut state = self.state.borrow_mut();
            let plan_path = plan_file_path(&bootstrap.state, &state.session_dir);
            state.plan_mode = crate::plan_mode::PiPlanTracker::with_plan_file(plan_path);
        }
        // Mirror load_session: push Pi's baseline contextUsage so the pager
        // context bar does not keep the previous session's numerator.
        if !cancelled {
            self.refresh_context_usage().await;
        }
        let mut response = acp::NewSessionResponse::new(bootstrap.state.session_id.clone());
        if let Some(models) = bootstrap.acp_models() {
            response = response.models(Some(models));
        }
        response = response.modes(Some(self.acp_session_modes()));
        self.persist_plan_mode_state().map_err(acp_internal)?;
        self.sync_plan_mode_control().map_err(acp_internal)?;
        self.publish_plan_file_path().await;
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
        {
            let mut state = self.state.borrow_mut();
            state.acp_session_id = requested.clone();
            let plan_path = plan_file_path(&bootstrap.state, &state.session_dir);
            state.plan_mode = load_plan_tracker(&plan_path).map_err(acp_internal)?;
        }
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
        response = response.modes(Some(self.acp_session_modes()));
        self.sync_plan_mode_control().map_err(acp_internal)?;
        self.publish_plan_file_path().await;
        Ok(response)
    }

    async fn set_session_mode(
        &self,
        arguments: acp::SetSessionModeRequest,
    ) -> Result<acp::SetSessionModeResponse, acp::Error> {
        let mode_id = arguments.mode_id.0.to_string();
        let mode = mode_id.as_str();
        let (changed, current_mode_id) = {
            let mut state = self.state.borrow_mut();
            let turn_in_flight = !state.active_prompts.is_empty();
            let changed = if mode == "plan" {
                state.plan_mode.enter_pending()
            } else {
                // Any non-plan mode exits plan mode (default/ask/agent).
                let was_plan = state.plan_mode.state() != crate::plan_mode::PiPlanState::Inactive;
                if was_plan {
                    state.plan_mode.user_exit(turn_in_flight);
                    true
                } else {
                    false
                }
            };
            // ACP display state follows the request. ExitPending is an internal
            // turn-drain state only; Pager must immediately confirm default.
            let current = if mode == "plan" { "plan" } else { mode };
            (changed, current.to_string())
        };
        if changed {
            self.persist_plan_mode_state().map_err(acp_internal)?;
            self.sync_plan_mode_control().map_err(acp_internal)?;
            self.send_update(acp::SessionUpdate::CurrentModeUpdate(
                acp::CurrentModeUpdate::new(acp::SessionModeId::new(current_mode_id)),
            ))
            .await;
        }
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
        let display_message = message.clone();
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
        let plan_file_to_seed = {
            let state = self.state.borrow();
            (state.plan_mode.state() == crate::plan_mode::PiPlanState::Pending)
                .then(|| state.plan_mode.plan_file_path().to_path_buf())
        };
        if let Some(plan_file) = plan_file_to_seed {
            ensure_plan_file(&plan_file).map_err(acp_internal)?;
        }

        enum Disposition {
            Queued {
                id: String,
            },
            Direct {
                operation_id: u64,
                message: String,
                streaming_behavior: Option<&'static str>,
                pin_primary_running: bool,
            },
        }

        let (completion_tx, completion_rx) = oneshot::channel();
        let disposition = {
            let mut state = self.state.borrow_mut();
            let streaming_behavior =
                prompt_streaming_behavior(Self::adapter_busy(&state), arguments.meta.as_ref());
            if streaming_behavior == Some("followUp") {
                let id = state.queue_mirror.enqueue_local(
                    client_prompt_id.clone(),
                    message,
                    display_message,
                    images.clone(),
                    QueueLane::FollowUp,
                    QueueOrigin::Client,
                );
                state
                    .queued_prompt_completions
                    .insert(id.clone(), completion_tx);
                Disposition::Queued { id }
            } else {
                let execution_message = Self::prepare_execution_text(&mut state, message);
                let operation_id = state.next_prompt_id;
                state.next_prompt_id = state.next_prompt_id.wrapping_add(1).max(1);
                if let Some(lane) = streaming_behavior.and_then(queue_lane_for_behavior)
                    && let Some(client_id) = client_prompt_id.as_deref()
                {
                    state.queue_mirror.reserve(
                        client_id.to_string(),
                        execution_message.clone(),
                        display_message.clone(),
                        images.clone(),
                        lane,
                        QueueOrigin::Client,
                    );
                }
                let pin_primary_running = streaming_behavior.is_none()
                    && client_prompt_id.as_deref().is_some_and(|id| !id.is_empty());
                if pin_primary_running {
                    let client_id = client_prompt_id.clone().expect("checked non-empty");
                    state.live_prompt_id = Some(client_id.clone());
                    state.queue_mirror.set_running_primary(
                        client_id,
                        execution_message.clone(),
                        display_message,
                        images.clone(),
                        QueueOrigin::Client,
                    );
                }
                state.agent_running = true;
                state.active_prompts.push(ActivePrompt {
                    id: operation_id,
                    client_prompt_id: client_prompt_id.clone(),
                    completion: completion_tx,
                    agent_started: false,
                    cancelled: false,
                });
                Disposition::Direct {
                    operation_id,
                    message: execution_message,
                    streaming_behavior,
                    pin_primary_running,
                }
            }
        };

        self.persist_plan_mode_state().map_err(acp_internal)?;
        self.sync_plan_mode_control().map_err(acp_internal)?;

        match disposition {
            Disposition::Queued { id } => {
                self.publish_queue_snapshot().await;
                self.dispatch_next_queued().await;
                let completion = completion_rx.await.unwrap_or(PromptCompletion {
                    reason: acp::StopReason::Cancelled,
                    client_prompt_id: Some(id.clone()),
                });
                Ok(prompt_response(
                    completion.reason,
                    completion.client_prompt_id.as_deref().or(Some(id.as_str())),
                ))
            }
            Disposition::Direct {
                operation_id,
                message,
                streaming_behavior,
                pin_primary_running,
            } => {
                let mut request = json!({ "type": "prompt", "message": message });
                if !images.is_empty() {
                    request["images"] = Value::Array(images);
                }
                if let Some(streaming_behavior) = streaming_behavior {
                    request["streamingBehavior"] = Value::String(streaming_behavior.to_string());
                }
                if let Err(error) = self.rpc.request(request).await {
                    if let Some(client_id) = client_prompt_id.as_deref() {
                        self.state
                            .borrow_mut()
                            .queue_mirror
                            .release_reservation(client_id);
                    }
                    self.remove_prompt(operation_id);
                    if pin_primary_running {
                        let mut state = self.state.borrow_mut();
                        state.agent_running = false;
                        state.live_prompt_id = None;
                        state.queue_mirror.clear_running();
                    }
                    self.publish_queue_snapshot().await;
                    return Err(acp_internal(error));
                }
                if pin_primary_running {
                    self.rebroadcast_queue_mirror().await;
                }
                let probe = self.clone();
                tokio::task::spawn_local(async move {
                    probe.probe_prompt_without_agent().await;
                });
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
        }
    }

    async fn cancel(&self, _arguments: acp::CancelNotification) -> Result<(), acp::Error> {
        let (command, queued, running) = {
            let mut state = self.state.borrow_mut();
            for active in &mut state.active_prompts {
                active.cancelled = true;
            }
            state.cancelling = true;
            let command = if state.bash_running {
                "abort_bash"
            } else {
                "abort"
            };
            let queued = state.queue_mirror.clear_local();
            let running = state.queue_mirror.clear_running();
            state.queue_mirror.clear();
            (command, queued, running)
        };

        self.finish_queued_entries(queued, acp::StopReason::Cancelled);
        self.finish_prompts(acp::StopReason::Cancelled);
        if let Some(entry) = running
            && entry.origin != QueueOrigin::Client
        {
            self.send_server_prompt_complete(&entry, acp::StopReason::Cancelled)
                .await;
        }
        self.publish_queue_snapshot().await;

        // Do not await Pi's abort RPC. AgentSession.abort() waits for idle, and
        // extension continuations can otherwise keep ACP cancel blocked forever.
        if let Err(error) = self.rpc.notify(json!({ "type": command })) {
            tracing::warn!(%error, "failed to notify Pi abort");
        }
        let probe = self.clone();
        tokio::task::spawn_local(async move {
            probe.settle_cancelled_prompts().await;
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
                self.handle_bash_background_request(arguments.params.get())
                    .await
            }
            "x.ai/task/kill" => self.handle_bash_kill_request(arguments.params.get()).await,
            "x.ai/recap" => self.handle_recap_request(arguments.params.get()).await,
            "x.ai/btw" => self.handle_btw_request(arguments.params.get()).await,
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
                let use_psm_index = params
                    .get("usePsmIndex")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                self.publish_session_catalog(cwd, all, use_psm_index).await;
                ext_response(json!({})).map_err(acp_internal)
            }
            // Full-text search across Pi sessions via PSM SQLite FTS5.
            "pi/session/search" => {
                let params: Value =
                    serde_json::from_str(arguments.params.get()).unwrap_or_default();
                let query = string(&params, &["query"])
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                let cwd = string(&params, &["cwd"]).filter(|c| !c.trim().is_empty());
                let limit = params.get("limit").and_then(Value::as_u64).unwrap_or(20) as usize;
                if query.is_empty() {
                    return ext_response(json!({ "results": [], "total": 0 }))
                        .map_err(acp_internal);
                }
                let cwd_path = cwd.map(PathBuf::from);
                let results = tokio::task::spawn_blocking(move || {
                    crate::psm_session_catalog::full_text_search(cwd_path.as_deref(), &query, limit)
                })
                .await
                .unwrap_or(None)
                .unwrap_or_default();
                let total = results.len();
                ext_response(json!({
                    "results": results.iter().map(|r| json!({
                        "sessionId": r.session_id,
                        "cwd": r.cwd,
                        "summary": r.summary,
                        "snippet": r.snippet,
                        "score": r.score,
                        "matchedFields": r.matched_fields,
                        "updatedAt": r.updated_at,
                    })).collect::<Vec<_>>(),
                    "total": total,
                }))
                .map_err(acp_internal)
            }
            // Session message preview for /resume picker (PSM message_entries).
            "pi/session/messages" => {
                let params: Value =
                    serde_json::from_str(arguments.params.get()).unwrap_or_default();
                let session_path = string(&params, &["sessionPath", "session_path"])
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                let session_id = string(&params, &["sessionId", "session_id"])
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                let limit = params.get("limit").and_then(Value::as_u64).unwrap_or(200) as usize;
                let messages = tokio::task::spawn_blocking(move || {
                    let path = if !session_path.is_empty() {
                        Some(session_path)
                    } else if !session_id.is_empty() {
                        crate::psm_session_catalog::resolve_session_path(&session_id)
                    } else {
                        None
                    };
                    match path {
                        Some(p) => crate::psm_session_catalog::load_session_messages(&p, limit)
                            .unwrap_or_default(),
                        None => Vec::new(),
                    }
                })
                .await
                .unwrap_or_default();
                ext_response(json!({
                    "messages": messages.iter().map(|m| json!({
                        "role": m.role,
                        "content": m.content,
                    })).collect::<Vec<_>>(),
                    "total": messages.len(),
                }))
                .map_err(acp_internal)
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
                let custom_instructions = string(&params, &["customInstructions", "instructions"]);
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
            // Pi /fork message catalog (RPC get_fork_messages).
            "pi/session/fork_messages" => {
                let data = self.fetch_fork_messages().await?;
                ext_response(data).map_err(acp_internal)
            }
            // Pi /fork: create branched session file from a user message entry.
            "pi/session/fork" => {
                let params: Value =
                    serde_json::from_str(arguments.params.get()).unwrap_or_default();
                let entry_id = string(&params, &["entryId", "id", "targetId"])
                    .map(str::trim)
                    .filter(|id| !id.is_empty())
                    .ok_or_else(|| acp::Error::invalid_params().data("entryId is required"))?;
                let data = self.fork_from_entry(entry_id).await?;
                ext_response(data).map_err(acp_internal)
            }
            // Pi /clone: duplicate current leaf into a new session file.
            "pi/session/clone" => {
                let data = self.clone_current_session().await?;
                ext_response(data).map_err(acp_internal)
            }
            // Pi /reload: settings + resources via injected ctx.reload().
            "pi/session/reload" => {
                let data = self.reload_session_resources().await?;
                ext_response(data).map_err(acp_internal)
            }
            // Queue delivery mode: set Pi's follow-up / steering drain mode.
            // "one-at-a-time" (default): deliver one queued message per turn.
            // "all": deliver all queued messages at once.
            "pi/queue/mode" => {
                let params: Value =
                    serde_json::from_str(arguments.params.get()).unwrap_or_default();
                let mode = string(&params, &["mode"])
                    .map(str::trim)
                    .filter(|m| *m == "all" || *m == "one-at-a-time")
                    .ok_or_else(|| {
                        acp::Error::invalid_params().data("mode must be 'all' or 'one-at-a-time'")
                    })?;
                if params.get("steering").and_then(Value::as_bool) == Some(true) {
                    self.rpc
                        .request(json!({ "type": "set_steering_mode", "mode": mode }))
                        .await
                        .map_err(acp_internal)?;
                } else {
                    self.rpc
                        .request(json!({ "type": "set_follow_up_mode", "mode": mode }))
                        .await
                        .map_err(acp_internal)?;
                }
                ext_response(json!({ "mode": mode })).map_err(acp_internal)
            }
            // Tree file rollback: preview via injected extension command.
            "pi/session/rollback_preview" => {
                let params: Value =
                    serde_json::from_str(arguments.params.get()).unwrap_or_default();
                let entry_id = string(&params, &["entryId", "id", "targetId"])
                    .map(str::trim)
                    .filter(|id| !id.is_empty())
                    .ok_or_else(|| acp::Error::invalid_params().data("entryId is required"))?;
                self.run_bridge_command("__pi_rollback_preview", entry_id)
                    .await?;
                ext_response(json!({ "entryId": entry_id })).map_err(acp_internal)
            }
            // Tree file rollback: execute via injected extension command.
            "pi/session/rollback_execute" => {
                let params: Value =
                    serde_json::from_str(arguments.params.get()).unwrap_or_default();
                let entry_id = string(&params, &["entryId", "id", "targetId"])
                    .map(str::trim)
                    .filter(|id| !id.is_empty())
                    .ok_or_else(|| acp::Error::invalid_params().data("entryId is required"))?;
                self.run_bridge_command("__pi_rollback_execute", entry_id)
                    .await?;
                ext_response(json!({ "entryId": entry_id, "executed": true })).map_err(acp_internal)
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
            "x.ai/workflows/list" => {
                let cwd = std::env::current_dir().ok();
                let listings = xai_grok_shell::session::workflow::list_workflows(cwd.as_deref());
                let workflows: Vec<Value> = listings
                    .into_iter()
                    .map(|w| {
                        json!({
                            "name": w.name,
                            "description": w.description,
                            "source": w.source,
                            "path": w.path,
                            "builtin": w.source == "builtin",
                        })
                    })
                    .collect();
                ext_response(json!({ "workflows": workflows })).map_err(acp_internal)
            }
            "x.ai/workflow/launch" => {
                let params: Value =
                    serde_json::from_str(arguments.params.get()).map_err(acp_internal)?;
                let name = string(&params, &["name", "workflow"])
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| acp::Error::invalid_params().data("name is required"))?;
                let args = string(&params, &["args", "input"]).unwrap_or("");
                let data = self.handle_workflow_request(name, args).await?;
                ext_response(data).map_err(acp_internal)
            }
            "x.ai/workflow/pause" => {
                let params: Value =
                    serde_json::from_str(arguments.params.get()).map_err(acp_internal)?;
                let run_id = string(&params, &["runId", "run_id", "name"])
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| acp::Error::invalid_params().data("runId is required"))?;
                let ok = self.workflow_pause(run_id).await?;
                ext_response(json!({ "runId": run_id, "paused": ok })).map_err(acp_internal)
            }
            "x.ai/workflow/stop" => {
                let params: Value =
                    serde_json::from_str(arguments.params.get()).map_err(acp_internal)?;
                let run_id = string(&params, &["runId", "run_id", "name"])
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| acp::Error::invalid_params().data("runId is required"))?;
                let ok = self.workflow_cancel(run_id).await?;
                ext_response(json!({ "runId": run_id, "stopped": ok })).map_err(acp_internal)
            }
            "x.ai/subagent/cancel" => {
                let params: Value =
                    serde_json::from_str(arguments.params.get()).map_err(acp_internal)?;
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
                let params: Value =
                    serde_json::from_str(arguments.params.get()).unwrap_or_default();
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
            // Native extension-shortcut dispatch (Pager match_key → RPC prompt →
            // hidden /__pi_shortcut_dispatch → extension handler).
            // Prefer RPC over keyfile; independent of remote-tui.
            "pi/ui/shortcut_dispatch" => {
                let params: Value =
                    serde_json::from_str(arguments.params.get()).unwrap_or_default();
                if let Some(key) = string(&params, &["key"]) {
                    let key = key.trim();
                    if !key.is_empty() {
                        let message = bridge_command_message(SHORTCUT_DISPATCH_COMMAND, key);
                        if let Err(error) = self
                            .rpc
                            .request(json!({ "type": "prompt", "message": message }))
                            .await
                        {
                            tracing::warn!(%error, %key, "shortcut_dispatch RPC prompt failed");
                            // Fallback: legacy keyfile if env/meta present.
                            if let Err(file_err) = append_shortcut_dispatch_event(json!({
                                "op": "dispatch",
                                "key": key,
                            })) {
                                tracing::debug!(%file_err, "shortcut_dispatch keyfile fallback failed");
                            }
                        }
                    }
                }
                Ok(())
            }
            "x.ai/queue/remove" => {
                let params: Value =
                    serde_json::from_str(arguments.params.get()).unwrap_or_default();
                let id = string(&params, &["id"]).unwrap_or_default();
                let expected_version = params
                    .get("expectedVersion")
                    .or_else(|| params.get("version"))
                    .and_then(Value::as_u64);
                let removed = self
                    .state
                    .borrow_mut()
                    .queue_mirror
                    .take_local(id, expected_version);
                if let Some(entry) = removed {
                    self.finish_queued_entries(vec![entry], acp::StopReason::Cancelled);
                } else {
                    self.send_ui_notification(
                        "This queue row is already running or belongs to Pi's external queue",
                        Some("warning"),
                    )
                    .await;
                }
                self.publish_queue_snapshot().await;
                Ok(())
            }
            "x.ai/queue/clear" => {
                let removed = self.state.borrow_mut().queue_mirror.clear_local();
                self.finish_queued_entries(removed, acp::StopReason::Cancelled);
                self.publish_queue_snapshot().await;
                Ok(())
            }
            "x.ai/queue/edit" => {
                let params: Value =
                    serde_json::from_str(arguments.params.get()).unwrap_or_default();
                let id = string(&params, &["id"]).unwrap_or_default();
                let new_text = string(&params, &["newText", "text"])
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .map(str::to_string);
                let edited = new_text
                    .is_some_and(|text| self.state.borrow_mut().queue_mirror.edit_local(id, text));
                if !edited {
                    self.send_ui_notification(
                        "Only pending adapter queue rows can be edited",
                        Some("warning"),
                    )
                    .await;
                }
                self.publish_queue_snapshot().await;
                Ok(())
            }
            "x.ai/queue/reorder" => {
                let params: Value =
                    serde_json::from_str(arguments.params.get()).unwrap_or_default();
                let ordered_ids = params
                    .get("orderedIds")
                    .or_else(|| params.get("ids"))
                    .and_then(Value::as_array)
                    .map(|values| {
                        values
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                self.state
                    .borrow_mut()
                    .queue_mirror
                    .reorder_local(&ordered_ids);
                self.publish_queue_snapshot().await;
                Ok(())
            }
            "x.ai/queue/interject" => {
                let params: Value =
                    serde_json::from_str(arguments.params.get()).unwrap_or_default();
                let id = string(&params, &["id"]).unwrap_or_default();
                let expected_version = params
                    .get("expectedVersion")
                    .or_else(|| params.get("version"))
                    .and_then(Value::as_u64);
                let new_text = string(&params, &["newText", "text"])
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .map(str::to_string);
                if !self
                    .interject_local_queue(id, expected_version, new_text)
                    .await
                {
                    self.send_ui_notification(
                        "Only a pending adapter queue row can be sent now",
                        Some("warning"),
                    )
                    .await;
                    self.publish_queue_snapshot().await;
                }
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
        let control_meta = self.bash_control_meta.as_deref().ok_or_else(|| {
            acp::Error::invalid_params().data("Pi Bash background control is disabled")
        })?;
        append_bash_background_control(control_meta, tool_call_id).map_err(acp_internal)?;
        ext_response(json!({ "accepted": true, "terminalId": tool_call_id })).map_err(acp_internal)
    }

    /// Kill a Pi-owned background Bash task via the private control channel.
    ///
    /// Pager clicks the native task-card kill control and sends `x.ai/task/kill`.
    /// The adapter only validates the task id against the extension-published
    /// `runningTaskIds` set and appends a control event; the extension owns the
    /// child process and emits `task_completed` after the kill settles.
    async fn handle_bash_kill_request(
        &self,
        params_raw: &str,
    ) -> Result<acp::ExtResponse, acp::Error> {
        let params: Value = serde_json::from_str(params_raw).map_err(acp_internal)?;
        let task_id = string(&params, &["taskId", "task_id"])
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| acp::Error::invalid_params().data("taskId is required"))?;
        let control_meta = self.bash_control_meta.as_deref().ok_or_else(|| {
            acp::Error::invalid_params().data("Pi Bash background control is disabled")
        })?;
        let outcome = append_bash_kill_control(control_meta, task_id).map_err(acp_internal)?;
        // `ext_response` wraps the payload under `result`, matching
        // `ExtMethodResult<KillTaskResponse>` expected by Pager.
        ext_response(json!({
            "taskId": task_id,
            "outcome": outcome,
        }))
        .map_err(acp_internal)
    }

    /// Fire-and-forget session recap via injected `__pi_grok_recap` extension.
    ///
    /// Params: `{ sessionId?, auto?, model?, models?, customInstructions? }`.
    /// Language is taken from process locale.
    async fn handle_recap_request(&self, params_raw: &str) -> Result<acp::ExtResponse, acp::Error> {
        let params: Value = serde_json::from_str(params_raw).unwrap_or_else(|_| json!({}));
        let auto = params.get("auto").and_then(Value::as_bool).unwrap_or(false);
        let reserved = {
            let mut state = self.state.borrow_mut();
            reserve_recap_request(&mut state.recap_in_flight)
        };
        if !reserved {
            return ext_response(json!({ "ok": true, "auto": auto, "skipped": "in_flight" }))
                .map_err(acp_internal);
        }
        let models = model_chain_from_params(&params);
        let model = models.first().cloned().or_else(|| {
            string(&params, &["model", "modelId", "recapModel"])
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToOwned::to_owned)
        });
        let thinking_level = string(&params, &["thinkingLevel", "thinking_level"])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned);
        let recap_mermaid = params
            .get("recapMermaid")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let terminal_width = params
            .get("terminalWidth")
            .or_else(|| params.get("terminal_width"))
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let custom_instructions = string(
            &params,
            &["customInstructions", "custom_instructions", "instructions"],
        )
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned);
        let language = system_language_tag();
        let payload = json!({
            "auto": auto,
            "model": model,
            "models": models,
            "thinkingLevel": thinking_level,
            "recapMermaid": recap_mermaid,
            "terminalWidth": terminal_width,
            "language": language,
            "customInstructions": custom_instructions,
        });
        let args = payload.to_string();
        // Extension emits custom message asynchronously; adapter projects it.
        // Await preflight so extension errors surface before we ack.
        let result = self.run_bridge_command(RECAP_COMMAND, &args).await;
        self.state.borrow_mut().recap_in_flight = false;
        result?;
        ext_response(json!({ "ok": true, "auto": auto })).map_err(acp_internal)
    }

    /// Blocking side question via injected `__pi_grok_btw` extension.
    ///
    /// Params: `{ sessionId?, question, models?[] }`.
    /// Returns `{ result: { answer } }` matching stock Grok pager parse path.
    async fn handle_btw_request(&self, params_raw: &str) -> Result<acp::ExtResponse, acp::Error> {
        if !btw_extension_enabled() {
            return Err(acp::Error::method_not_found().data(
                "Native /btw is off. F2 → Agent → Pi /btw → on, fully quit, then restart grok-pi.",
            ));
        }
        let params: Value = serde_json::from_str(params_raw).map_err(acp_internal)?;
        let question = string(&params, &["question", "text", "q"])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned)
            .ok_or_else(|| acp::Error::invalid_params().data("question is required"))?;
        let models = model_chain_from_params(&params);
        let thinking_level = string(&params, &["thinkingLevel", "thinking_level"])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned);
        let request_id = format!("btw-{}", uuid::Uuid::now_v7());
        let (tx, rx) = oneshot::channel();
        self.state
            .borrow_mut()
            .pending_btw
            .insert(request_id.clone(), tx);
        let payload = json!({
            "requestId": request_id.clone(),
            "question": question,
            "models": models,
            "thinkingLevel": thinking_level,
        });
        if let Err(error) = self
            .run_bridge_command(BTW_COMMAND, &payload.to_string())
            .await
        {
            self.state.borrow_mut().pending_btw.remove(&request_id);
            return Err(error);
        }
        let result = match tokio::time::timeout(Duration::from_secs(120), rx).await {
            Ok(Ok(Ok(answer))) => Ok(answer),
            Ok(Ok(Err(error))) => Err(error),
            Ok(Err(_)) => Err("side question channel closed".into()),
            Err(_) => {
                self.state.borrow_mut().pending_btw.remove(&request_id);
                Err("side question timed out".into())
            }
        };
        match result {
            Ok(answer) => ext_response(json!({ "answer": answer })).map_err(acp_internal),
            Err(error) => Err(acp::Error::internal_error().data(error)),
        }
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
        let client_id = string(&params, &["interjectionId", "promptId"])
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_string);
        if let Some(client_id) = client_id.as_deref() {
            self.state.borrow_mut().queue_mirror.reserve(
                client_id.to_string(),
                message.clone(),
                message.clone(),
                images.clone(),
                QueueLane::Steering,
                QueueOrigin::Client,
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
        let data = match self.rpc.request(request).await {
            Ok(data) => data,
            Err(error) => {
                if let Some(client_id) = client_id.as_deref() {
                    self.state
                        .borrow_mut()
                        .queue_mirror
                        .release_reservation(client_id);
                }
                return Err(acp_internal(error));
            }
        };
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
const RELOAD_COMMAND: &str = "__pi_reload";
const SUBAGENT_CANCEL_COMMAND: &str = "__pi_grok_subagent_cancel";
const RECAP_COMMAND: &str = "__pi_grok_recap";
const BTW_COMMAND: &str = "__pi_grok_btw";
const CONTEXT_BREAKDOWN_COMMAND: &str = "__pi_context_breakdown";
const SHORTCUT_DISPATCH_COMMAND: &str = "__pi_shortcut_dispatch";

fn is_bridge_command(name: &str) -> bool {
    name.eq_ignore_ascii_case(NAVIGATE_TREE_COMMAND)
        || name.eq_ignore_ascii_case(LABEL_TREE_COMMAND)
        || name.eq_ignore_ascii_case(RELOAD_COMMAND)
        || name.eq_ignore_ascii_case(SUBAGENT_CANCEL_COMMAND)
        || name.eq_ignore_ascii_case(RECAP_COMMAND)
        || name.eq_ignore_ascii_case(BTW_COMMAND)
        || name.eq_ignore_ascii_case(CONTEXT_BREAKDOWN_COMMAND)
        || name.eq_ignore_ascii_case(SHORTCUT_DISPATCH_COMMAND)
        || name.eq_ignore_ascii_case(WORKFLOW_SPAWN_COMMAND)
        || name.eq_ignore_ascii_case(WORKFLOW_CANCEL_COMMAND)
}

/// Ordered non-empty model refs from `models` array and/or legacy `model` field.
fn model_chain_from_params(params: &Value) -> Vec<String> {
    let mut out = Vec::new();
    let mut push = |raw: &str| {
        let t = raw.trim();
        if !t.is_empty() && !out.iter().any(|x: &String| x == t) {
            out.push(t.to_owned());
        }
    };
    if let Some(arr) = params.get("models").and_then(Value::as_array) {
        for item in arr {
            if let Some(s) = item.as_str() {
                push(s);
            }
        }
    }
    if let Some(s) = string(params, &["model", "modelId", "recapModel", "btwModel"]) {
        push(s);
    }
    out
}

fn recap_extension_enabled() -> bool {
    env_flag_enabled(std::env::var("PI_GROK_RECAP").ok().as_deref())
}

fn env_flag_enabled(value: Option<&str>) -> bool {
    match value.map(str::trim) {
        Some("1") => true,
        Some(value) => value.eq_ignore_ascii_case("true") || value.eq_ignore_ascii_case("on"),
        None => false,
    }
}

fn btw_extension_enabled() -> bool {
    if let Ok(config) = xai_grok_shell::config::load_effective_config() {
        if config
            .get("ui")
            .and_then(|ui| ui.get("pi_btw"))
            .and_then(|v| v.as_bool())
            == Some(true)
        {
            return true;
        }
    }
    match std::env::var("PI_GROK_BTW") {
        Ok(v) => {
            let v = v.trim();
            v == "1" || v.eq_ignore_ascii_case("true") || v.eq_ignore_ascii_case("on")
        }
        Err(_) => false,
    }
}

fn bridge_command_is_registered(commands: &[PiCommand], command: &str) -> bool {
    let command = command.trim_start_matches('/');
    commands.iter().any(|available| {
        available
            .name
            .trim_start_matches('/')
            .eq_ignore_ascii_case(command)
    })
}

fn reserve_recap_request(in_flight: &mut bool) -> bool {
    if *in_flight {
        return false;
    }
    *in_flight = true;
    true
}

fn bridge_command_message(command: &str, args: &str) -> String {
    if args.trim().is_empty() {
        format!("/{command}")
    } else {
        format!("/{command} {args}")
    }
}

/// Operating-system language for recap output.
///
/// On macOS, `AppleLanguages` is the authoritative Language & Region order;
/// terminal locale variables can remain `C` or differ from the UI language.
/// Other platforms (and macOS fallback) use the standard locale variables.
fn system_language_tag() -> Option<String> {
    #[cfg(target_os = "macos")]
    if let Ok(output) = std::process::Command::new("defaults")
        .args(["read", "-g", "AppleLanguages"])
        .output()
        && output.status.success()
        && let Some(language) = first_apple_language(&String::from_utf8_lossy(&output.stdout))
    {
        return Some(language);
    }

    for key in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(value) = std::env::var(key)
            && let Some(language) = normalize_language_tag(&value)
        {
            return Some(language);
        }
    }
    None
}

fn first_apple_language(value: &str) -> Option<String> {
    value
        .split(|character: char| {
            character == '(' || character == ')' || character == ',' || character.is_whitespace()
        })
        .find_map(normalize_language_tag)
}

fn normalize_language_tag(value: &str) -> Option<String> {
    let tag = value
        .trim()
        .trim_matches(|character| character == '"' || character == '\'')
        .split('.')
        .next()
        .unwrap_or_default()
        .replace('_', "-");
    if tag.is_empty() || tag.eq_ignore_ascii_case("C") || tag.eq_ignore_ascii_case("POSIX") {
        None
    } else {
        Some(tag)
    }
}

fn command_catalog(commands: &[PiCommand]) -> Vec<acp::AvailableCommand> {
    // The adapter reports Pi's command catalog (normalized + deduped), minus
    // private bridge commands. When Pi workflows are enabled, inject the
    // upstream-aligned workflow slash surface so Pager autocomplete matches
    // stock Grok: /workflow, /create-workflow, and named workflow scripts.
    let mut seen = HashSet::new();
    let mut out: Vec<acp::AvailableCommand> = commands
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
            let mut meta = serde_json::Map::new();
            if !command.source.trim().is_empty() {
                meta.insert(
                    "piCommandSource".to_string(),
                    Value::String(command.source.clone()),
                );
            }
            if !command.argument_completions.is_empty() {
                meta.insert(
                    "piArgumentCompletions".to_string(),
                    Value::Array(
                        command
                            .argument_completions
                            .iter()
                            .map(|item| {
                                json!({
                                    "value": item.value,
                                    "label": item.label,
                                    "description": item.description,
                                })
                            })
                            .collect(),
                    ),
                );
            }
            let mut available = acp::AvailableCommand::new(name.to_string(), description);
            if let Some(hint) = command
                .argument_hint
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                available = available.input(Some(acp::AvailableCommandInput::Unstructured(
                    acp::UnstructuredCommandInput::new(hint.to_string()),
                )));
            }
            if !meta.is_empty() {
                available = available.meta(meta);
            }
            Some(available)
        })
        .collect();

    if workflows_extension_enabled_static() {
        inject_workflow_slash_commands(&mut out, &mut seen);
    }
    out
}

fn workflows_extension_enabled_static() -> bool {
    if let Ok(config) = xai_grok_shell::config::load_effective_config() {
        if config
            .get("ui")
            .and_then(|ui| ui.get("pi_workflows"))
            .and_then(|v| v.as_bool())
            == Some(true)
        {
            return true;
        }
    }
    match std::env::var("PI_GROK_WORKFLOWS") {
        Ok(v) => {
            let v = v.trim();
            v == "1" || v.eq_ignore_ascii_case("true") || v.eq_ignore_ascii_case("on")
        }
        Err(_) => false,
    }
}

fn inject_workflow_slash_commands(
    out: &mut Vec<acp::AvailableCommand>,
    seen: &mut HashSet<String>,
) {
    let push = |out: &mut Vec<acp::AvailableCommand>,
                seen: &mut HashSet<String>,
                name: &str,
                description: &str,
                hint: Option<&str>| {
        let key = name.to_ascii_lowercase();
        if !seen.insert(key) {
            return;
        }
        let mut cmd = acp::AvailableCommand::new(name.to_string(), description.to_string());
        if let Some(hint) = hint {
            cmd = cmd.input(Some(acp::AvailableCommandInput::Unstructured(
                acp::UnstructuredCommandInput::new(hint.to_string()),
            )));
        }
        out.push(cmd);
    };

    push(
        out,
        seen,
        "workflow",
        "Launch a saved workflow, or manage a run (pause, resume, stop, save)",
        Some("<name> [args] | pause|resume|stop|save [name]"),
    );
    push(
        out,
        seen,
        "workflows",
        "Show workflow runs (phases, agents, progress)",
        None,
    );
    push(
        out,
        seen,
        "create-workflow",
        "Author a new multi-agent workflow",
        Some("[goal]"),
    );

    // Named project/user/builtin scripts as first-class slash entries.
    let cwd = std::env::current_dir().ok();
    let listings = xai_grok_shell::session::workflow::list_workflows(cwd.as_deref());
    for listing in listings {
        let desc = format!("Workflow: {}", listing.description);
        push(out, seen, &listing.name, &desc, Some("<args>"));
    }
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

/// Derive a plan sidecar that belongs to precisely one Pi JSONL session.
///
/// Pi's session store contains files, not Grok-style per-session directories;
/// `<session>.plan.md` avoids sharing a bare `plan.md` across all sessions.
/// The fallback is still session-id namespaced when Pi has not materialized a
/// session file yet.
fn plan_file_path(state: &PiState, configured_dir: &Path) -> PathBuf {
    if let Some(session_file) = state
        .session_file
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        return PathBuf::from(session_file).with_extension("plan.md");
    }
    configured_dir
        .join("grok-pi-plans")
        .join(format!("{}.plan.md", state.session_id))
}

/// Ensure activation has a writable, empty plan artifact without truncating a
/// previous plan on re-entry.
fn ensure_plan_file(path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("plan file has no parent directory: {}", path.display()))?;
    std::fs::create_dir_all(parent)
        .map_err(|error| anyhow!("create plan directory {}: {error}", parent.display()))?;
    if path.exists() {
        if !path.is_file() {
            bail!("plan path is not a regular file: {}", path.display());
        }
        return Ok(());
    }
    std::fs::File::create(path)
        .map_err(|error| anyhow!("create plan file {}: {error}", path.display()))?;
    Ok(())
}

fn plan_state_path(plan_file: &Path) -> PathBuf {
    let name = plan_file
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("plan.md");
    let base = name.strip_suffix(".plan.md").unwrap_or(name);
    plan_file.with_file_name(format!("{base}.plan-mode.json"))
}

fn load_plan_tracker(plan_file: &Path) -> Result<crate::plan_mode::PiPlanTracker> {
    let state_path = plan_state_path(plan_file);
    match std::fs::read(&state_path) {
        Ok(bytes) => {
            let snapshot: crate::plan_mode::PiPlanSnapshot = serde_json::from_slice(&bytes)
                .map_err(|error| {
                    anyhow!("parse plan-mode state {}: {error}", state_path.display())
                })?;
            Ok(
                crate::plan_mode::PiPlanTracker::from_snapshot_with_plan_file(
                    plan_file.to_path_buf(),
                    snapshot,
                ),
            )
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(
            crate::plan_mode::PiPlanTracker::with_plan_file(plan_file.to_path_buf()),
        ),
        Err(error) => Err(anyhow!(
            "read plan-mode state {}: {error}",
            state_path.display()
        )),
    }
}

fn atomic_write(path: &Path, body: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("state path has no parent directory: {}", path.display()))?;
    std::fs::create_dir_all(parent)
        .map_err(|error| anyhow!("create state directory {}: {error}", parent.display()))?;
    let staged = parent.join(format!(
        ".{}.{}.next",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("plan-mode"),
        std::process::id(),
    ));
    std::fs::write(&staged, body)
        .map_err(|error| anyhow!("write staged state {}: {error}", staged.display()))?;
    std::fs::rename(&staged, path)
        .map_err(|error| anyhow!("replace state {}: {error}", path.display()))?;
    Ok(())
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

/// Ask the injected Bash extension to kill a running background task.
///
/// Returns the wire outcome string consumed by Pager (`killed` / `not_found`).
/// The extension is the process owner; this only validates against the published
/// `runningTaskIds` set and appends a one-way control event.
fn append_bash_kill_control(meta_path: &Path, task_id: &str) -> Result<&'static str> {
    use std::fs::OpenOptions;
    use std::io::Write;

    let meta: Value = serde_json::from_str(&std::fs::read_to_string(meta_path)?)?;
    let running = meta
        .get("runningTaskIds")
        .and_then(Value::as_array)
        .is_some_and(|ids| ids.iter().any(|id| id.as_str() == Some(task_id)));
    if !running {
        return Ok("not_found");
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
        serde_json::to_string(&json!({ "op": "kill", "taskId": task_id }))?
    )?;
    Ok("killed")
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

/// Extension-shortcut dispatch keyfile (pi-grok-shortcut-manager watches it).
/// Prefer instance env `PI_GROK_SHORTCUT_KEYS` (set by grok-pi). Fall back to
/// legacy meta `tmp/pi-grok-shortcut-dispatch-active.json` for older children.
fn append_shortcut_dispatch_event(event: Value) -> Result<()> {
    use std::fs::OpenOptions;
    use std::io::Write;

    let keys_path = if let Ok(path) = std::env::var("PI_GROK_SHORTCUT_KEYS") {
        if path.is_empty() { None } else { Some(path) }
    } else {
        None
    };
    let keys_path = match keys_path {
        Some(path) => path,
        None => {
            let meta_path = std::env::temp_dir().join("pi-grok-shortcut-dispatch-active.json");
            if !meta_path.exists() {
                bail!("shortcut_dispatch keys missing (set PI_GROK_SHORTCUT_KEYS)");
            }
            let meta: Value = serde_json::from_str(&std::fs::read_to_string(&meta_path)?)?;
            meta.get("keysPath")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("shortcut_dispatch meta missing keysPath"))?
                .to_string()
        }
    };
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&keys_path)?;
    writeln!(file, "{}", serde_json::to_string(&event)?)?;
    file.flush()?;
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

const PI_GROK_MULTI_SELECT_TITLE_PREFIX: &str = "__pi_grok_multi_select_v1__:";
const PI_GROK_RESOURCE_PICKER_TITLE_PREFIX: &str = "__pi_grok_resource_picker_v1__:";

/// Decode a narrow, product-owned request to render a normal Pi `ui.select`
/// callback with QuestionView's native checkbox mode. The payload is not a new
/// Pi RPC surface: it is an opt-in title envelope understood only by the
/// injected grok-pi subagent extension.
fn extension_multi_select_title(title: &str) -> Option<String> {
    let encoded = title.strip_prefix(PI_GROK_MULTI_SELECT_TITLE_PREFIX)?;
    serde_json::from_str::<Value>(encoded)
        .ok()?
        .get("title")?
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

/// Decode the product-owned select envelope that opens the existing native Pi
/// resource manager in selection mode. This travels over the ordinary
/// extension UI and ACP question request; it does not extend Pi RPC.
fn extension_resource_picker(title: &str) -> Option<Value> {
    let encoded = title.strip_prefix(PI_GROK_RESOURCE_PICKER_TITLE_PREFIX)?;
    let value = serde_json::from_str::<Value>(encoded).ok()?;
    let object = value.as_object()?;
    object
        .get("title")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|title| !title.is_empty())?;
    let types = object.get("resourceTypes")?.as_array()?;
    (!types.is_empty() && types.iter().all(Value::is_string)).then_some(value)
}

const ASK_USER_CANCEL_TEXT: &str = "User declined to answer the questions. Continue with the task using your best judgment, or ask different questions.";

fn write_ask_user_response(tool_call_id: &str, payload: Value) {
    let Some(dir) = std::env::var_os("PI_GROK_ASK_USER_DIR") else {
        tracing::warn!(
            tool_call_id,
            "PI_GROK_ASK_USER_DIR unset; cannot write Q&A response"
        );
        return;
    };
    let path = std::path::Path::new(&dir).join(format!("{tool_call_id}.json"));
    if let Err(error) = std::fs::write(&path, payload.to_string()) {
        tracing::warn!(%error, path = %path.display(), "failed to write ask_user_question response");
    }
}

/// Normalize model/tool args into the ACP Question array for QuestionView.
fn normalize_ask_user_questions(args: Option<&Value>) -> Vec<Value> {
    let Some(args) = args else {
        return Vec::new();
    };
    let Some(items) = args.get("questions").and_then(Value::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let question = item
                .get("question")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|text| !text.is_empty())?;
            let header = item
                .get("header")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|text| !text.is_empty());
            let question_text = match header {
                Some(header) => format!("{header}: {question}"),
                None => question.to_string(),
            };
            let options = item
                .get("options")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|option| {
                    let label = option
                        .get("label")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|text| !text.is_empty())?;
                    let description = option
                        .get("description")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let preview = option.get("preview").cloned().filter(|v| !v.is_null());
                    Some(json!({
                        "label": label,
                        "description": description,
                        "preview": preview,
                        "id": null,
                    }))
                })
                .collect::<Vec<_>>();
            if options.len() < 2 {
                return None;
            }
            let multi_select = item
                .get("multi_select")
                .or_else(|| item.get("multiSelect"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            Some(json!({
                "question": question_text,
                "options": options,
                "multiSelect": multi_select,
                "id": null,
            }))
        })
        .collect()
}

/// Format Grok QuestionView outcome into the extension control payload.
fn format_ask_user_tool_result(result: &Value) -> Value {
    let outcome = result
        .get("outcome")
        .and_then(Value::as_str)
        .unwrap_or("cancelled");
    match outcome {
        "accepted" => {
            let Some(answers) = result.get("answers").and_then(Value::as_object) else {
                return json!({
                    "outcome": "cancelled",
                    "message": ASK_USER_CANCEL_TEXT,
                });
            };
            let annotations = result.get("annotations").and_then(Value::as_object);
            let entries: Vec<String> = answers
                .iter()
                .map(|(question, selected)| {
                    let labels = match selected {
                        Value::String(s) => s.clone(),
                        Value::Array(items) => items
                            .iter()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join(", "),
                        _ => selected.to_string(),
                    };
                    let mut parts = vec![format!("\"{question}\"=\"{labels}\"")];
                    if let Some(ann) = annotations.and_then(|map| map.get(question)) {
                        if let Some(preview) = ann.get("preview").and_then(Value::as_str) {
                            parts.push(format!("selected preview:\n{preview}"));
                        }
                        if let Some(notes) = ann.get("notes").and_then(Value::as_str) {
                            parts.push(format!("user notes: {notes}"));
                        }
                    }
                    parts.join(" ")
                })
                .collect();
            let message = format!(
                "User has answered your questions: {}. You can now continue with the user's answers in mind.",
                entries.join(", ")
            );
            json!({ "outcome": "accepted", "message": message })
        }
        "chat_about_this" | "skip_interview" => {
            // Plan-mode partial paths: surface partial labels so the model can continue.
            let partial = result
                .get("partial_answers")
                .and_then(Value::as_object)
                .map(|map| {
                    map.iter()
                        .map(|(q, v)| format!("\"{q}\"=\"{}\"", v.as_str().unwrap_or_default()))
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .filter(|s| !s.is_empty());
            let message = match (outcome, partial) {
                ("chat_about_this", Some(partial)) => format!(
                    "User wants to chat about the plan interview. Partial answers: {partial}"
                ),
                ("chat_about_this", None) => {
                    "User wants to chat about the plan interview without selecting options."
                        .to_string()
                }
                (_, Some(partial)) => {
                    format!("User skipped the plan interview. Partial answers: {partial}")
                }
                _ => "User skipped the plan interview.".to_string(),
            };
            json!({ "outcome": "accepted", "message": message })
        }
        _ => json!({
            "outcome": "cancelled",
            "message": ASK_USER_CANCEL_TEXT,
        }),
    }
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

/// Pi's stock select callback accepts one string, so encode the native
/// QuestionView multi-select labels as a JSON array for the product-owned
/// callback envelope above. The TypeScript caller validates every label before
/// using it as a configuration toggle.
fn extension_multi_select_answer(value: &Value) -> Option<String> {
    let answers = value.get("answers").and_then(Value::as_object)?;
    let labels = answers.values().find_map(|answer| {
        answer.as_array().map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
    })?;
    serde_json::to_string(&labels).ok()
}

fn extension_resource_picker_answer(value: &Value) -> Option<String> {
    let paths = value.get("paths")?.as_array()?;
    let paths = paths
        .iter()
        .filter_map(Value::as_str)
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    serde_json::to_string(&paths).ok()
}

fn ext_response(value: Value) -> Result<acp::ExtResponse> {
    let raw = serde_json::value::to_raw_value(&json!({ "result": value }))?;
    Ok(acp::ExtResponse::new(raw.into()))
}

fn acp_internal(error: impl std::fmt::Display) -> acp::Error {
    acp::Error::internal_error().data(error.to_string())
}

/// Wall-clock UTC ms for ACP `_meta.agentTimestampMs` / stream anchors.
fn utc_now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_now_ms_is_positive() {
        assert!(utc_now_ms() > 0);
    }

    #[test]
    fn recap_capability_requires_an_enabled_bridge_extension() {
        assert!(!env_flag_enabled(None));
        assert!(!env_flag_enabled(Some("false")));
        assert!(env_flag_enabled(Some("1")));
        assert!(env_flag_enabled(Some("on")));
    }

    #[test]
    fn bridge_commands_require_registration_and_recap_is_not_reentrant() {
        let commands = vec![PiCommand {
            name: RECAP_COMMAND.into(),
            ..Default::default()
        }];
        assert!(bridge_command_is_registered(&commands, RECAP_COMMAND));
        assert!(!bridge_command_is_registered(&commands, BTW_COMMAND));

        let mut in_flight = false;
        assert!(reserve_recap_request(&mut in_flight));
        assert!(!reserve_recap_request(&mut in_flight));
        in_flight = false;
        assert!(reserve_recap_request(&mut in_flight));
    }

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
        assert!(
            pending
                .defer_if_targeted(&replay)
                .expect("defer target replay")
        );
        assert!(
            !pending
                .defer_if_targeted(&other_session)
                .expect("leave unrelated event live")
        );
        assert_eq!(
            pending
                .commit_if_target("session-next")
                .expect("commit")
                .len(),
            1
        );
        assert!(
            pending
                .commit_if_target("session-next")
                .expect("no transition")
                .is_empty()
        );
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
        assert!(
            pending
                .defer_if_targeted(&replay)
                .expect("defer target replay")
        );
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
    fn appends_kill_control_only_for_a_running_task() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let control_path = directory.path().join("control.jsonl");
        let meta_path = directory.path().join("control.json");
        std::fs::write(&control_path, "").expect("control file");
        std::fs::write(
            &meta_path,
            json!({
                "controlPath": control_path,
                "activeToolCallIds": [],
                "runningTaskIds": ["bash-1"],
            })
            .to_string(),
        )
        .expect("metadata file");

        assert_eq!(
            append_bash_kill_control(&meta_path, "bash-1").expect("kill running task"),
            "killed"
        );
        assert_eq!(
            std::fs::read_to_string(&control_path).expect("read control file"),
            "{\"op\":\"kill\",\"taskId\":\"bash-1\"}\n"
        );
        assert_eq!(
            append_bash_kill_control(&meta_path, "bash-missing").expect("unknown task"),
            "not_found"
        );
        assert_eq!(
            std::fs::read_to_string(&control_path).expect("read control file after not_found"),
            "{\"op\":\"kill\",\"taskId\":\"bash-1\"}\n"
        );
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
        assert!(
            !description.contains("[anthropic]"),
            "provider stays on left: {description}"
        );
        assert!(description.contains("ctx 200k"), "{description}");
        assert!(description.contains("out 64k"), "{description}");
        assert!(description.contains("api anth"), "{description}");
        assert!(description.contains("in txt+img"), "{description}");
        assert!(description.contains("⚡"), "{description}");
        assert!(description.contains("$1 / $5"), "{description}");
        let meta = info.meta.as_ref().expect("meta");
        assert_eq!(
            meta.get("provider").and_then(|v| v.as_str()),
            Some("anthropic")
        );
        assert_eq!(
            meta.get("modelId").and_then(|v| v.as_str()),
            Some("claude-haiku-4-5")
        );
        assert_eq!(
            meta.get("api").and_then(|v| v.as_str()),
            Some("anthropic-messages")
        );
        assert_eq!(
            meta.get("totalContextTokens").and_then(|v| v.as_u64()),
            Some(200_000)
        );
        assert_eq!(meta.get("maxTokens").and_then(|v| v.as_u64()), Some(64_000));
    }

    #[test]
    fn command_catalog_is_pi_owned_and_deduplicated() {
        let commands = vec![
            PiCommand {
                name: "/review".into(),
                description: "Review changes".into(),
                source: "extension".into(),
                ..Default::default()
            },
            PiCommand {
                name: "REVIEW".into(),
                description: "Duplicate".into(),
                source: "prompt".into(),
                ..Default::default()
            },
            PiCommand {
                name: "brief".into(),
                description: String::new(),
                source: "skill".into(),
                ..Default::default()
            },
            PiCommand {
                name: NAVIGATE_TREE_COMMAND.into(),
                description: "internal".into(),
                source: "extension".into(),
                ..Default::default()
            },
            PiCommand {
                name: LABEL_TREE_COMMAND.into(),
                description: "internal".into(),
                source: "extension".into(),
                ..Default::default()
            },
            PiCommand {
                name: RELOAD_COMMAND.into(),
                description: "internal".into(),
                source: "extension".into(),
                ..Default::default()
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
        assert!(!text.contains(RELOAD_COMMAND));
    }

    #[test]
    fn command_catalog_carries_pi_argument_completions_in_meta() {
        let commands = vec![PiCommand {
            name: "gapp".into(),
            description: "Manage Glimpse-APPs".into(),
            source: "extension".into(),
            argument_hint: Some("<list|open|...>".into()),
            argument_completions: vec![crate::model::PiArgumentCompletion {
                value: "list".into(),
                label: "List apps".into(),
                description: String::new(),
            }],
        }];
        let catalog = command_catalog(&commands);
        // Workflows may inject extra slash entries when enabled; assert the gapp row.
        let cmd = catalog
            .iter()
            .find(|c| c.name == "gapp")
            .expect("gapp command present");
        match cmd.input.as_ref() {
            Some(acp::AvailableCommandInput::Unstructured(u)) => {
                assert_eq!(u.hint, "<list|open|...>");
            }
            other => panic!("expected unstructured input, got {other:?}"),
        }
        let comps = cmd
            .meta
            .as_ref()
            .and_then(|m| m.get("piArgumentCompletions"))
            .and_then(|v| v.as_array())
            .expect("piArgumentCompletions meta");
        assert_eq!(comps.len(), 1);
        assert_eq!(comps[0]["value"], "list");
        assert_eq!(comps[0]["label"], "List apps");
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
    fn normalize_ask_user_questions_maps_multi_select_and_header() {
        let args = json!({
            "questions": [{
                "header": "Auth",
                "question": "Which method?",
                "multi_select": true,
                "options": [
                    { "label": "JWT (Recommended)", "description": "stateless" },
                    { "label": "Session", "description": "cookie" }
                ]
            }]
        });
        let questions = normalize_ask_user_questions(Some(&args));
        assert_eq!(questions.len(), 1);
        assert_eq!(questions[0]["question"], "Auth: Which method?");
        assert_eq!(questions[0]["multiSelect"], true);
        assert_eq!(questions[0]["options"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn format_ask_user_tool_result_accepted_and_cancel() {
        let accepted = format_ask_user_tool_result(&json!({
            "outcome": "accepted",
            "answers": { "Which DB?": ["Postgres"] },
            "annotations": { "Which DB?": { "notes": "managed" } }
        }));
        assert_eq!(accepted["outcome"], "accepted");
        let message = accepted["message"].as_str().unwrap();
        assert!(message.contains("Which DB?"));
        assert!(message.contains("Postgres"));
        assert!(message.contains("user notes: managed"));

        let cancelled = format_ask_user_tool_result(&json!({ "outcome": "cancelled" }));
        assert_eq!(cancelled["outcome"], "cancelled");
        assert!(cancelled["message"].as_str().unwrap().contains("declined"));
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
    fn product_multi_select_envelope_uses_native_checkbox_answer_shape() {
        assert_eq!(
            extension_multi_select_title(
                "__pi_grok_multi_select_v1__:{\"title\":\"Built-in tools\",\"maxSelections\":3}"
            ),
            Some("Built-in tools".into())
        );
        assert_eq!(extension_multi_select_title("ordinary select"), None);
        assert_eq!(
            extension_multi_select_answer(&json!({
                "answers": { "pi-question": ["☐ read", "☑ bash"] }
            })),
            Some("[\"☐ read\",\"☑ bash\"]".into())
        );
    }

    #[test]
    fn product_resource_picker_envelope_round_trips_selected_paths() {
        let picker = extension_resource_picker(
            "__pi_grok_resource_picker_v1__:{\"title\":\"Extensions\",\"resourceTypes\":[\"extensions\"],\"initialPaths\":[\"/tmp/a.ts\"]}",
        )
        .expect("valid resource-picker envelope");
        assert_eq!(picker["title"], "Extensions");
        assert!(extension_resource_picker("ordinary select").is_none());
        assert_eq!(
            extension_resource_picker_answer(&json!({
                "paths": ["/tmp/a.ts", "/tmp/b.ts"]
            })),
            Some("[\"/tmp/a.ts\",\"/tmp/b.ts\"]".into())
        );
    }

    #[test]
    fn normalizes_system_language_tags() {
        assert_eq!(normalize_language_tag("zh_CN.UTF-8"), Some("zh-CN".into()));
        assert_eq!(normalize_language_tag("\"en-US\""), Some("en-US".into()));
        assert_eq!(normalize_language_tag("C"), None);
        assert_eq!(normalize_language_tag("POSIX"), None);
    }

    #[test]
    fn parses_first_macos_preferred_language() {
        assert_eq!(
            first_apple_language("(\n    \"zh-Hans-CN\",\n    \"en-CN\"\n)"),
            Some("zh-Hans-CN".into())
        );
        assert_eq!(first_apple_language("(\n)"), None);
    }

    #[test]
    fn plan_sidecar_is_scoped_to_jsonl_session() {
        let state = PiState {
            session_id: "session-1".into(),
            session_file: Some("/tmp/pi/project/session.jsonl".into()),
            ..PiState::default()
        };
        let plan = plan_file_path(&state, Path::new("/tmp/pi"));
        assert_eq!(plan, PathBuf::from("/tmp/pi/project/session.plan.md"));
        assert_eq!(
            plan_state_path(&plan),
            PathBuf::from("/tmp/pi/project/session.plan-mode.json")
        );
    }

    #[test]
    fn plan_tracker_persists_and_restores_active_state() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let plan_file = directory.path().join("session.plan.md");
        let mut tracker = crate::plan_mode::PiPlanTracker::with_plan_file(plan_file.clone());
        tracker.enter_pending();
        tracker.build_reminder_for_prompt();
        atomic_write(
            &plan_state_path(&plan_file),
            &serde_json::to_vec(&tracker.snapshot()).expect("serialize snapshot"),
        )
        .expect("persist snapshot");

        let restored = load_plan_tracker(&plan_file).expect("restore snapshot");
        assert!(restored.is_active());
        assert_eq!(restored.plan_file_path(), plan_file.as_path());
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
