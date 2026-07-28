import { randomUUID } from "node:crypto";
import { dirname, join } from "node:path";
import { Type } from "typebox";
import {
  createAgentSession,
  DefaultResourceLoader,
  getAgentDir,
  SessionManager,
  SettingsManager,
  type AgentSession,
  type AgentSessionEvent,
  type ExtensionAPI,
  type ExtensionContext,
} from "@earendil-works/pi-coding-agent";

const BRIDGE_TYPE = "pi-grok-subagent/v1";
const STATE_ENTRY_TYPE = "pi-grok-subagent-state/v1";
const PROGRESS_INTERVAL_MS = 2_000;
const MAX_BACKGROUND_CONCURRENCY = 4;
const MAX_WAIT_MS = 600_000; // 10 minutes cap for blocking waits
const POLL_INTERVAL_MS = 500;

const CAPABILITY_TOOLS = {
  "read-only": ["read"],
  "read-write": ["read", "edit", "write"],
  execute: ["read", "bash"],
  all: ["read", "bash", "edit", "write"],
} as const;

type CapabilityMode = keyof typeof CAPABILITY_TOOLS;

const AGENT_PROFILES: Record<string, { capabilityMode: CapabilityMode; systemPrompt: string }> = {
  "general-purpose": {
    capabilityMode: "all",
    systemPrompt: "You are a focused coding subagent. Complete only the delegated task and return a concise evidence-based result.",
  },
  explore: {
    capabilityMode: "execute",
    systemPrompt: "You are a read-only exploration subagent. Inspect the codebase, run safe diagnostic commands, and report evidence without editing files.",
  },
  plan: {
    capabilityMode: "execute",
    systemPrompt: "You are a planning subagent. Inspect the codebase and return an implementation plan with risks and verification steps. Do not edit files.",
  },
};

type ChildUpdate =
  | { type: "assistant_delta"; text: string }
  | { type: "thinking_delta"; text: string }
  | { type: "user"; text: string }
  | { type: "tool_call"; toolCallId: string; toolName: string; args: unknown }
  | { type: "tool_update"; toolCallId: string; toolName: string; partialResult: unknown }
  | { type: "tool_result"; toolCallId: string; toolName: string; result: unknown; isError: boolean };

type BridgeKind = "spawned" | "progress" | "child_update" | "finished";

type BridgeEnvelope = {
  version: 1;
  sequence: number;
  replay: boolean;
  kind: BridgeKind;
  parentSessionId: string;
  subagentId: string;
  childSessionId: string;
  payload: Record<string, unknown>;
};

type PersistedRecord = {
  version: 1;
  id: string;
  childSessionId: string;
  childSessionFile: string;
  parentSessionId: string;
  parentToolCallId: string;
  prompt: string;
  description: string;
  type: string;
  capabilityMode: CapabilityMode;
  modelId: string;
  background: boolean;
  startedAt: number;
  status: "running" | "completed" | "failed" | "cancelled";
  turnCount: number;
  toolCallCount: number;
  tokensUsed: number;
};

type SubagentRecord = {
  id: string;
  childSessionId: string;
  childSessionFile: string;
  parentSessionId: string;
  parentToolCallId: string;
  prompt: string;
  description: string;
  type: string;
  capabilityMode: CapabilityMode;
  modelId: string;
  background: boolean;
  startedAt: number;
  session: AgentSession;
  turnCount: number;
  toolCallCount: number;
  toolsUsed: Set<string>;
  errorCount: number;
  tokensUsed: number;
  finished: boolean;
  /** Terminal status set by finish(): "completed" | "failed" | "cancelled". */
  terminalStatus: "completed" | "failed" | "cancelled" | null;
  /** Error message from finish(), if the subagent failed. */
  lastError?: string;
  cancelRequested: boolean;
  /** Max turns before injecting a summary prompt. 0 = unlimited. */
  maxTurns: number;
  /** Set when turn limit triggers abort-then-summarize. */
  turnLimitReached: boolean;
  /** Resolved when finish() is called — enables true blocking wait. */
  donePromise: Promise<void>;
  doneResolve: () => void;
  progressTimer: ReturnType<typeof setInterval>;
  removeAbortListener: () => void;
  unsubscribe: () => void;
};

function requireText(value: unknown, name: string): string {
  if (typeof value !== "string" || value.trim().length === 0) {
    throw new Error(`${name} is required`);
  }
  return value.trim();
}

function requireCapability(value: string | undefined): CapabilityMode {
  const capability = value ?? "all";
  if (!(capability in CAPABILITY_TOOLS)) {
    throw new Error(`unsupported capability_mode: ${capability}`);
  }
  return capability as CapabilityMode;
}

function resolveProfile(type: string, capabilityMode: string | undefined): {
  type: string;
  capabilityMode: CapabilityMode;
  systemPrompt: string;
} {
  const normalizedType = type.trim() || "general-purpose";
  const profile = AGENT_PROFILES[normalizedType] ?? AGENT_PROFILES["general-purpose"];
  return {
    type: normalizedType,
    capabilityMode: requireCapability(capabilityMode ?? profile.capabilityMode),
    systemPrompt: profile.systemPrompt,
  };
}

function textFromContent(content: unknown): string {
  if (typeof content === "string") return content;
  if (!Array.isArray(content)) return "";
  return content
    .filter((block): block is { type: string; text?: unknown } => typeof block === "object" && block !== null)
    .filter((block) => block.type === "text" && typeof block.text === "string")
    .map((block) => block.text as string)
    .join("");
}

function lastAssistantText(session: AgentSession): string {
  for (let index = session.messages.length - 1; index >= 0; index -= 1) {
    const message = session.messages[index];
    if (message.role !== "assistant") continue;
    return message.content
      .filter((block) => block.type === "text")
      .map((block) => block.text)
      .join("")
      .trim();
  }
  return "";
}

function extractUsage(message: unknown): number {
  if (typeof message !== "object" || message === null) return 0;
  const usage = (message as { usage?: unknown }).usage;
  if (typeof usage !== "object" || usage === null) return 0;
  const input = (usage as { input?: unknown }).input;
  const output = (usage as { output?: unknown }).output;
  return (typeof input === "number" ? input : 0) + (typeof output === "number" ? output : 0);
}

function childUpdate(event: AgentSessionEvent): ChildUpdate | undefined {
  if (event.type === "message_update") {
    if (event.assistantMessageEvent.type === "text_delta") {
      return { type: "assistant_delta", text: event.assistantMessageEvent.delta };
    }
    if (event.assistantMessageEvent.type === "thinking_delta") {
      return { type: "thinking_delta", text: event.assistantMessageEvent.delta };
    }
  }
  if (event.type === "tool_execution_start") {
    return {
      type: "tool_call",
      toolCallId: event.toolCallId,
      toolName: event.toolName,
      args: event.args,
    };
  }
  if (event.type === "tool_execution_update") {
    return {
      type: "tool_update",
      toolCallId: event.toolCallId,
      toolName: event.toolName,
      partialResult: event.partialResult,
    };
  }
  if (event.type === "tool_execution_end") {
    return {
      type: "tool_result",
      toolCallId: event.toolCallId,
      toolName: event.toolName,
      result: event.result,
      isError: event.isError,
    };
  }
  return undefined;
}

export default function piGrokSubagents(pi: ExtensionAPI): void {
  if (process.env.PI_GROK_SUBAGENTS !== "1") return;

  const records = new Map<string, SubagentRecord>();
  const queuedBackground: Array<{ record: SubagentRecord; prompt: string }> = [];
  let runningBackground = 0;
  let nextSequence = 1;

  function emit(
    record: Pick<SubagentRecord, "id" | "childSessionId" | "parentSessionId">,
    kind: BridgeKind,
    payload: Record<string, unknown>,
    replay = false,
  ): void {
    const envelope: BridgeEnvelope = {
      version: 1,
      sequence: nextSequence,
      replay,
      kind,
      parentSessionId: record.parentSessionId,
      subagentId: record.id,
      childSessionId: record.childSessionId,
      payload,
    };
    nextSequence += 1;
    if (replay) {
      // Replay runs during session_start. Keep the existing message shape for
      // the adapter, but do not append replay records to the active session.
      pi.sendMessage(
        {
          customType: BRIDGE_TYPE,
          content: "",
          display: false,
          details: envelope,
        },
        { triggerTurn: false },
      );
      return;
    }
    // Live bridge traffic is session/TUI state, not an LLM message. Using
    // sendMessage() here would steer the parent while it is streaming, even
    // with triggerTurn:false, causing child deltas to create phantom turns.
    pi.appendEntry(BRIDGE_TYPE, envelope);
  }

  function persistedRecord(entry: unknown): PersistedRecord | undefined {
    if (typeof entry !== "object" || entry === null) return undefined;
    const candidate = entry as { type?: unknown; customType?: unknown; data?: unknown };
    if (candidate.type !== "custom" || candidate.customType !== STATE_ENTRY_TYPE) return undefined;
    if (typeof candidate.data !== "object" || candidate.data === null) return undefined;
    const value = candidate.data as Partial<PersistedRecord>;
    if (
      value.version !== 1 ||
      typeof value.id !== "string" ||
      typeof value.childSessionId !== "string" ||
      typeof value.childSessionFile !== "string" ||
      typeof value.parentSessionId !== "string" ||
      typeof value.parentToolCallId !== "string" ||
      typeof value.prompt !== "string" ||
      typeof value.description !== "string" ||
      typeof value.type !== "string" ||
      typeof value.capabilityMode !== "string" ||
      typeof value.modelId !== "string" ||
      typeof value.background !== "boolean" ||
      typeof value.startedAt !== "number" ||
      typeof value.status !== "string" ||
      typeof value.turnCount !== "number" ||
      typeof value.toolCallCount !== "number" ||
      typeof value.tokensUsed !== "number"
    ) {
      return undefined;
    }
    return value as PersistedRecord;
  }

  function replayChildTranscript(snapshot: PersistedRecord): void {
    let entries: readonly unknown[];
    try {
      entries = SessionManager.open(snapshot.childSessionFile).getBranch();
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      emit(snapshot, "finished", {
        status: "failed",
        durationMs: 0,
        turns: snapshot.turnCount,
        toolCalls: snapshot.toolCallCount,
        tokensUsed: snapshot.tokensUsed,
        error: `child transcript is unavailable: ${message}`,
      }, true);
      return;
    }

    for (const entry of entries) {
      const message = (entry as { type?: unknown; message?: unknown }).message;
      if (typeof message !== "object" || message === null) continue;
      const childMessage = message as {
        role?: unknown;
        content?: unknown;
        toolCallId?: unknown;
        toolName?: unknown;
        isError?: unknown;
      };
      if (childMessage.role === "user") {
        emit(snapshot, "child_update", {
          update: { type: "user", text: textFromContent(childMessage.content) },
        }, true);
        continue;
      }
      if (childMessage.role === "assistant" && Array.isArray(childMessage.content)) {
        for (const block of childMessage.content) {
          if (typeof block !== "object" || block === null) continue;
          const value = block as { type?: unknown; text?: unknown; thinking?: unknown; id?: unknown; name?: unknown; arguments?: unknown };
          if (value.type === "text" && typeof value.text === "string") {
            emit(snapshot, "child_update", { update: { type: "assistant_delta", text: value.text } }, true);
          } else if (value.type === "thinking" && typeof value.thinking === "string") {
            emit(snapshot, "child_update", { update: { type: "thinking_delta", text: value.thinking } }, true);
          } else if (value.type === "toolCall" && typeof value.id === "string" && typeof value.name === "string") {
            emit(snapshot, "child_update", {
              update: { type: "tool_call", toolCallId: value.id, toolName: value.name, args: value.arguments ?? {} },
            }, true);
          }
        }
        continue;
      }
      if (
        childMessage.role === "toolResult" &&
        typeof childMessage.toolCallId === "string" &&
        typeof childMessage.toolName === "string"
      ) {
        emit(snapshot, "child_update", {
          update: {
            type: "tool_result",
            toolCallId: childMessage.toolCallId,
            toolName: childMessage.toolName,
            result: { content: childMessage.content },
            isError: childMessage.isError === true,
          },
        }, true);
      }
    }
  }

  function replayPersistedRecords(ctx: ExtensionContext): void {
    const latest = new Map<string, PersistedRecord>();
    for (const entry of ctx.sessionManager.getEntries()) {
      const snapshot = persistedRecord(entry);
      if (snapshot) latest.set(snapshot.id, snapshot);
    }
    const parentSessionId = ctx.sessionManager.getSessionId();
    for (const snapshot of [...latest.values()].sort((left, right) => left.startedAt - right.startedAt)) {
      if (snapshot.parentSessionId !== parentSessionId) continue;
      emit(snapshot, "spawned", {
        parentToolCallId: snapshot.parentToolCallId,
        description: snapshot.description,
        subagentType: snapshot.type,
        background: snapshot.background,
        capabilityMode: snapshot.capabilityMode,
        model: snapshot.modelId,
        prompt: snapshot.prompt,
      }, true);
      replayChildTranscript(snapshot);
      const status = snapshot.status === "running" ? "cancelled" : snapshot.status;
      emit(snapshot, "finished", {
        status,
        durationMs: Math.max(0, Date.now() - snapshot.startedAt),
        turns: snapshot.turnCount,
        toolCalls: snapshot.toolCallCount,
        tokensUsed: snapshot.tokensUsed,
        error: snapshot.status === "running" ? "Pi host restarted before child completion" : undefined,
      }, true);
    }
  }

  function persist(record: SubagentRecord, status: PersistedRecord["status"]): void {
    const snapshot: PersistedRecord = {
      version: 1,
      id: record.id,
      childSessionId: record.childSessionId,
      childSessionFile: record.childSessionFile,
      parentSessionId: record.parentSessionId,
      parentToolCallId: record.parentToolCallId,
      prompt: record.prompt,
      description: record.description,
      type: record.type,
      capabilityMode: record.capabilityMode,
      modelId: record.modelId,
      background: record.background,
      startedAt: record.startedAt,
      status,
      turnCount: record.turnCount,
      toolCallCount: record.toolCallCount,
      tokensUsed: record.tokensUsed,
    };
    pi.appendEntry(STATE_ENTRY_TYPE, snapshot);
  }

  function emitProgress(record: SubagentRecord): void {
    if (record.finished) return;
    emit(record, "progress", {
      durationMs: Date.now() - record.startedAt,
      turnCount: record.turnCount,
      toolCallCount: record.toolCallCount,
      toolsUsed: [...record.toolsUsed],
      errorCount: record.errorCount,
      tokensUsed: record.tokensUsed,
    });
  }

  function finish(record: SubagentRecord, status: "completed" | "failed" | "cancelled", error?: string): void {
    if (record.finished) return;
    record.finished = true;
    record.terminalStatus = status;
    if (error) record.lastError = error;
    clearInterval(record.progressTimer);
    record.removeAbortListener();
    record.unsubscribe();
    record.doneResolve();
    persist(record, status);
    emit(record, "finished", {
      status,
      durationMs: Date.now() - record.startedAt,
      turns: record.turnCount,
      toolCalls: record.toolCallCount,
      tokensUsed: record.tokensUsed,
      error,
      output: lastAssistantText(record.session),
    });
  }

  async function createRecord(
    toolCallId: string,
    params: {
      prompt: string;
      description: string;
      subagent_type: string;
      background?: boolean;
      capability_mode?: string;
      max_turns?: number;
    },
    signal: AbortSignal | undefined,
    ctx: ExtensionContext,
  ): Promise<SubagentRecord> {
    const prompt = requireText(params.prompt, "prompt");
    const description = requireText(params.description, "description");
    const profile = resolveProfile(params.subagent_type || "general-purpose", params.capability_mode);
    const model = ctx.model;
    if (!model) throw new Error("no Pi model is selected");

    const parentSessionFile = ctx.sessionManager.getSessionFile();
    if (!parentSessionFile) throw new Error("parent session persistence is unavailable");

    const agentDir = getAgentDir();
    const settingsManager = SettingsManager.create(ctx.cwd, agentDir);
    const resourceLoader = new DefaultResourceLoader({
      cwd: ctx.cwd,
      agentDir,
      noExtensions: true,
      noSkills: true,
      noPromptTemplates: true,
      noThemes: true,
      noContextFiles: true,
      systemPromptOverride: () => profile.systemPrompt,
      appendSystemPromptOverride: () => [],
    });
    await resourceLoader.reload();

    const { session } = await createAgentSession({
      cwd: ctx.cwd,
      agentDir,
      sessionManager: SessionManager.create(ctx.cwd, join(dirname(parentSessionFile), "subagent")),
      settingsManager,
      modelRegistry: ctx.modelRegistry,
      model,
      tools: [...CAPABILITY_TOOLS[profile.capabilityMode]],
      resourceLoader,
    });
    await session.bindExtensions({});

    const childSessionFile = session.sessionFile;
    if (!childSessionFile) throw new Error("child session persistence is unavailable");
    const parentSessionId = ctx.sessionManager.getSessionId();
    const id = randomUUID();
    let doneResolve!: () => void;
    const donePromise = new Promise<void>((resolve) => { doneResolve = resolve; });
    const record = {
      id,
      childSessionId: session.sessionId,
      childSessionFile,
      parentSessionId,
      parentToolCallId: toolCallId,
      prompt,
      description,
      type: profile.type,
      capabilityMode: profile.capabilityMode,
      modelId: model.id,
      background: params.background === true,
      startedAt: Date.now(),
      session,
      turnCount: 0,
      toolCallCount: 0,
      toolsUsed: new Set<string>(),
      errorCount: 0,
      tokensUsed: 0,
      finished: false,
      terminalStatus: null,
      cancelRequested: false,
      maxTurns: params.max_turns ?? 0,
      turnLimitReached: false,
      donePromise,
      doneResolve,
    } as Omit<SubagentRecord, "progressTimer" | "removeAbortListener" | "unsubscribe">;

    const unsubscribe = session.subscribe((event) => {
      if (event.type === "turn_end") {
        record.turnCount += 1;
        // Turn limit reached: steer the agent with a summary prompt.
        // "steer" interrupts the current turn and injects the message,
        // giving the agent a chance to produce final output naturally.
        if (record.maxTurns > 0 && record.turnCount >= record.maxTurns && !record.turnLimitReached) {
          record.turnLimitReached = true;
          const summaryPrompt =
            "[SYSTEM] You have reached the maximum number of turns allowed (" +
            String(record.maxTurns) +
            "). Stop all further tool calls immediately. " +
            "Produce your final summary now — return a concise, evidence-based result " +
            "of everything you have gathered so far. Do not make any more tool calls.";
          void session.prompt(summaryPrompt, { streamingBehavior: "steer" }).catch(() => undefined);
        }
      }
      if (event.type === "tool_execution_start") {
        record.toolCallCount += 1;
        record.toolsUsed.add(event.toolName);
      }
      if (event.type === "tool_execution_end" && event.isError) record.errorCount += 1;
      if (event.type === "message_end" && event.message.role === "assistant") {
        record.tokensUsed += extractUsage(event.message);
      }
      const update = childUpdate(event);
      if (update) emit(record, "child_update", { update });
    });

    const onAbort = () => {
      record.cancelRequested = true;
      session.abort();
    };
    signal?.addEventListener("abort", onAbort, { once: true });
    const removeAbortListener = () => signal?.removeEventListener("abort", onAbort);
    const progressTimer = setInterval(() => emitProgress(record as SubagentRecord), PROGRESS_INTERVAL_MS);
    const completeRecord: SubagentRecord = { ...record, progressTimer, removeAbortListener, unsubscribe };

    records.set(id, completeRecord);
    persist(completeRecord, "running");
    emit(completeRecord, "spawned", {
      parentToolCallId: toolCallId,
      description,
      subagentType: completeRecord.type,
      background: completeRecord.background,
      capabilityMode: profile.capabilityMode,
      model: model.id,
      prompt,
    });
    emit(completeRecord, "child_update", { update: { type: "user", text: prompt } });
    return completeRecord;
  }

  async function run(record: SubagentRecord, prompt: string): Promise<string> {
    try {
      await record.session.prompt(prompt);
      const output = lastAssistantText(record.session);
      finish(record, "completed");
      return output;
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      finish(record, record.cancelRequested ? "cancelled" : "failed", message);
      throw error;
    }
  }

  pi.on("session_start", (_event, ctx) => {
    replayPersistedRecords(ctx);
  });

  function scheduleBackground(record: SubagentRecord, prompt: string): void {
    if (runningBackground >= MAX_BACKGROUND_CONCURRENCY) {
      queuedBackground.push({ record, prompt });
      return;
    }
    runningBackground += 1;
    void run(record, prompt)
      .catch(() => undefined)
      .finally(() => {
        runningBackground -= 1;
        const next = queuedBackground.shift();
        if (next) scheduleBackground(next.record, next.prompt);
      });
  }

  // ---------------------------------------------------------------------------
  // Helpers for output formatting
  // ---------------------------------------------------------------------------

  function formatDuration(ms: number): string {
    if (ms < 1000) return `${ms}ms`;
    const seconds = Math.floor(ms / 1000);
    if (seconds < 60) return `${seconds}s`;
    const minutes = Math.floor(seconds / 60);
    return `${minutes}m${seconds % 60}s`;
  }

  function statusLabel(record: SubagentRecord): string {
    if (!record.finished) return "RUNNING";
    return (record.terminalStatus ?? "completed").toUpperCase();
  }

  function formatSubagentResult(record: SubagentRecord): string {
    const elapsed = formatDuration(Date.now() - record.startedAt);
    const status = statusLabel(record);
    const header = `[${status}] ${record.description} (${record.id.slice(0, 8)}…) — ${elapsed}, ${record.turnCount} turns, ${record.toolCallCount} tool calls`;
    if (!record.finished) {
      const tools = [...record.toolsUsed].join(", ") || "none yet";
      return `${header}\nStatus: still running. Tools used: ${tools}. Tokens: ${record.tokensUsed}.\nUse get_command_or_subagent_output with timeout_ms to wait for completion.`;
    }
    const output = lastAssistantText(record.session);
    const errorLine = record.lastError ? `\nError: ${record.lastError}` : "";
    if (!output) return `${header}${errorLine}\n(Subagent completed without text output.)`;
    // Truncate very long outputs to avoid flooding parent context
    const MAX_OUTPUT_CHARS = 12_000;
    const truncated = output.length > MAX_OUTPUT_CHARS
      ? `${output.slice(0, MAX_OUTPUT_CHARS)}\n\n… [truncated ${output.length - MAX_OUTPUT_CHARS} chars]`
      : output;
    return `${header}${errorLine}\n\n${truncated}`;
  }

  function waitForRecords(ids: string[], timeoutMs: number, signal?: AbortSignal): Promise<void> {
    const promises = ids.map((id) => {
      const r = records.get(id);
      return r ? r.donePromise : Promise.resolve();
    });
    const all = Promise.all(promises).then(() => undefined);
    if (timeoutMs <= 0) return all;
    const timeout = new Promise<void>((resolve) => {
      const timer = setTimeout(resolve, timeoutMs);
      signal?.addEventListener("abort", () => { clearTimeout(timer); resolve(); }, { once: true });
    });
    return Promise.race([all, timeout]);
  }

  // ---------------------------------------------------------------------------
  // Tool: spawn_subagent
  // ---------------------------------------------------------------------------

  pi.registerTool({
    name: "spawn_subagent",
    label: "Spawn Subagent",
    description:
      "Launch an autonomous Pi child session shown in Grok's native subagent UI.\n\n" +
      "Usage notes:\n" +
      "- Set background=true to run asynchronously; returns the subagent ID immediately\n" +
      "- For background subagents, use get_command_or_subagent_output with task_ids and timeout_ms to wait for results\n" +
      "- Without background (default), blocks until the subagent finishes and returns its final output directly\n" +
      "- Do NOT use wait_tasks for subagent IDs — use get_command_or_subagent_output instead\n" +
      "- You can spawn multiple background subagents in parallel (up to 4 concurrent)",
    parameters: Type.Object({
      prompt: Type.String({ description: "Self-contained task for the child agent. Include all context needed — the child cannot see your conversation." }),
      description: Type.String({ description: "Short 3-5 word task label shown in the subagent UI." }),
      subagent_type: Type.Optional(Type.String({ description: "Agent profile: general-purpose (default), explore (read-only research), or plan (planning only)." })),
      background: Type.Optional(Type.Boolean({ description: "Run asynchronously and return the child ID immediately. Use get_command_or_subagent_output(task_ids, timeout_ms) to collect results." })),
      capability_mode: Type.Optional(
        Type.String({ description: "Tool access: read-only, read-write, execute, or all. Defaults to profile capability." }),
      ),
    }),
    async execute(toolCallId, params, signal, _onUpdate, ctx) {
      const record = await createRecord(toolCallId, params, signal, ctx);
      if (record.background) {
        scheduleBackground(record, params.prompt);
        return {
          content: [{ type: "text", text: `Started background subagent ${record.id}.\nUse get_command_or_subagent_output with task_ids=["${record.id}"] and timeout_ms to wait for its result.` }],
          details: { subagentId: record.id, childSessionId: record.childSessionId, background: true },
        };
      }
      const output = await run(record, params.prompt);
      return {
        content: [{ type: "text", text: output || "Subagent completed without text output." }],
        details: { subagentId: record.id, childSessionId: record.childSessionId, background: false },
      };
    },
  });

  // ---------------------------------------------------------------------------
  // Tool: get_command_or_subagent_output
  // ---------------------------------------------------------------------------

  pi.registerTool({
    name: "get_command_or_subagent_output",
    label: "Get Subagent Output",
    description:
      "Get output and status from one or more background subagents.\n\n" +
      "Usage notes:\n" +
      "- Pass task_ids with one or more subagent IDs from background=true spawn_subagent calls\n" +
      "- For a single subagent use a one-element array: task_ids=[\"<id>\"]\n" +
      "- Set a positive timeout_ms to block until all listed subagents complete (or timeout). Recommended: 120000–600000\n" +
      "- Omit timeout_ms or pass 0 for a non-blocking status snapshot\n" +
      "- Returns status, progress, and final output text for each subagent\n" +
      "- Do NOT use wait_tasks for subagent IDs — this tool handles waiting",
    parameters: Type.Object({
      task_ids: Type.Optional(Type.Array(Type.String(), { description: "One or more subagent IDs to check." })),
      subagent_id: Type.Optional(Type.String({ description: "Single subagent ID (alternative to task_ids for one subagent)." })),
      timeout_ms: Type.Optional(Type.Number({ description: "Max milliseconds to wait for completion. 0 or omitted = non-blocking snapshot. Capped at 600000 (10 min)." })),
    }),
    async execute(_toolCallId, params, signal) {
      // Accept both task_ids array and legacy subagent_id single string
      const ids: string[] = params.task_ids?.length
        ? params.task_ids
        : params.subagent_id
          ? [params.subagent_id]
          : [];
      if (ids.length === 0) throw new Error("Provide task_ids (array) or subagent_id (string) with at least one subagent ID");

      // Validate all IDs exist
      const unknown = ids.filter((id) => !records.has(id));
      if (unknown.length > 0) {
        throw new Error(`unknown subagent(s): ${unknown.join(", ")}. Use list_subagents to see active subagents.`);
      }

      // Blocking wait if timeout_ms > 0
      const timeoutMs = Math.min(Math.max(params.timeout_ms ?? 0, 0), MAX_WAIT_MS);
      if (timeoutMs > 0) {
        await waitForRecords(ids, timeoutMs, signal);
      }

      // Format results
      const results = ids.map((id) => {
        const record = records.get(id)!;
        return formatSubagentResult(record);
      });

      const allFinished = ids.every((id) => records.get(id)!.finished);
      const summary = allFinished
        ? "All subagents finished."
        : "Some subagents still running. Call again with a larger timeout_ms to wait longer.";

      return {
        content: [{ type: "text", text: `${summary}\n\n${results.join("\n\n---\n\n")}` }],
        details: {
          subagents: ids.map((id) => {
            const r = records.get(id)!;
            return { subagentId: id, finished: r.finished, status: statusLabel(r), turns: r.turnCount, toolCalls: r.toolCallCount };
          }),
        },
      };
    },
  });

  // ---------------------------------------------------------------------------
  // Tool: kill_command_or_subagent
  // ---------------------------------------------------------------------------

  pi.registerTool({
    name: "kill_command_or_subagent",
    label: "Cancel Subagent",
    description: "Cancel a running background subagent by ID. The subagent will be aborted and marked as cancelled.",
    parameters: Type.Object({
      task_id: Type.Optional(Type.String({ description: "The subagent ID to cancel." })),
      subagent_id: Type.Optional(Type.String({ description: "The subagent ID to cancel (alternative to task_id)." })),
    }),
    async execute(_toolCallId, params) {
      const id = requireText(params.task_id ?? params.subagent_id, "task_id or subagent_id");
      const record = records.get(id);
      if (!record) throw new Error(`unknown subagent: ${id}`);
      if (record.finished) {
        return { content: [{ type: "text", text: `Subagent ${id.slice(0, 8)}… already finished (${statusLabel(record)}).` }] };
      }
      record.cancelRequested = true;
      record.session.abort();
      return { content: [{ type: "text", text: `Cancelled subagent ${id.slice(0, 8)}… (${record.description}).` }] };
    },
  });

  // ---------------------------------------------------------------------------
  // Tool: list_subagents
  // ---------------------------------------------------------------------------

  pi.registerTool({
    name: "list_subagents",
    label: "List Subagents",
    description: "List all subagents in this session with their current status, progress, and IDs.",
    parameters: Type.Object({}),
    async execute() {
      if (records.size === 0) {
        return { content: [{ type: "text", text: "No subagents have been spawned in this session." }] };
      }
      const lines = [...records.values()]
        .sort((a, b) => a.startedAt - b.startedAt)
        .map((r) => {
          const elapsed = formatDuration(Date.now() - r.startedAt);
          const status = statusLabel(r);
          const bg = r.background ? "bg" : "fg";
          return `• [${status}] ${r.id.slice(0, 8)}… "${r.description}" (${bg}, ${r.type}) — ${elapsed}, ${r.turnCount} turns, ${r.toolCallCount} tools`;
        });
      return { content: [{ type: "text", text: `Subagents (${records.size}):\n${lines.join("\n")}` }] };
    },
  });

  // ---------------------------------------------------------------------------
  // Internal command (kept for backward compat with pager bridge)
  // ---------------------------------------------------------------------------

  pi.registerCommand("__pi_grok_subagent_cancel", {
    description: "Internal Pi-Grok bridge command: cancel a subagent",
    handler: async (args) => {
      const id = requireText(args, "subagent id");
      const record = records.get(id);
      if (!record) throw new Error(`unknown subagent: ${id}`);
      if (!record.finished) {
        record.cancelRequested = true;
        record.session.abort();
      }
    },
  });

  pi.on("session_shutdown", () => {
    for (const record of records.values()) {
      if (!record.finished) {
        record.cancelRequested = true;
        record.session.abort();
      }
    }
  });
}
