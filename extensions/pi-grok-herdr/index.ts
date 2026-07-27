// Built into grok-pi. Reports Pi lifecycle state to the Herdr local socket.
import net from "node:net";

const HERDR_ENV = process.env.HERDR_ENV;
const HERDR_SOCKET_PATH = process.env.HERDR_SOCKET_PATH;
const HERDR_PANE_ID = process.env.HERDR_PANE_ID;
const SOURCE = "herdr:pi";
const AGENT = "pi";
const SOCKET_ENDPOINT =
  process.platform === "win32" && HERDR_SOCKET_PATH
    ? `\\\\.\\pipe\\${HERDR_SOCKET_PATH}`
    : HERDR_SOCKET_PATH;

function isEnabled() {
  return HERDR_ENV === "1" && Boolean(SOCKET_ENDPOINT) && Boolean(HERDR_PANE_ID);
}

function requestId(kind) {
  return `${SOURCE}:${kind}:${Date.now()}:${Math.random().toString(36).slice(2)}`;
}

function deliverOnce(request, timeoutMs) {
  if (!isEnabled()) return Promise.resolve(true);

  return new Promise((resolve) => {
    const socket = net.createConnection(SOCKET_ENDPOINT);
    let settled = false;
    let timer;

    const finish = (delivered) => {
      if (settled) return;
      settled = true;
      if (timer) clearTimeout(timer);
      socket.destroy();
      resolve(delivered);
    };

    socket.on("error", () => finish(false));
    socket.on("connect", () => socket.write(`${JSON.stringify(request)}\n`));
    socket.on("data", () => finish(true));
    socket.on("end", () => finish(false));
    timer = setTimeout(() => finish(false), timeoutMs);
    timer.unref?.();
  });
}

async function deliver(request) {
  if (await deliverOnce(request, 500)) return;
  await deliverOnce(request, 1500);
}

let sequence = Date.now() * 1000;
let sessionPath;
let sessionId;

function nextSequence() {
  sequence += 1;
  return sequence;
}

function refreshSession(ctx) {
  try {
    const candidate = ctx?.sessionManager?.getSessionFile?.();
    sessionPath =
      typeof candidate === "string" && candidate.startsWith("/")
        ? candidate
        : undefined;
  } catch {
    sessionPath = undefined;
  }

  try {
    const candidate = ctx?.sessionManager?.getSessionId?.();
    sessionId =
      typeof candidate === "string" && candidate.length > 0 ? candidate : undefined;
  } catch {
    sessionId = undefined;
  }
}

function sessionFields() {
  if (sessionPath) return { agent_session_path: sessionPath };
  if (sessionId) return { agent_session_id: sessionId };
  return {};
}

async function reportSession(reason) {
  const fields = sessionFields();
  if (Object.keys(fields).length === 0) return;

  await deliver({
    id: requestId("session"),
    method: "pane.report_agent_session",
    params: {
      pane_id: HERDR_PANE_ID,
      source: SOURCE,
      agent: AGENT,
      seq: nextSequence(),
      session_start_source: reason,
      ...fields,
    },
  });
}

async function reportState(state, message, seq) {
  await deliver({
    id: requestId("state"),
    method: "pane.report_agent",
    params: {
      pane_id: HERDR_PANE_ID,
      source: SOURCE,
      agent: AGENT,
      state,
      message,
      seq,
      ...sessionFields(),
    },
  });
}

let sending = false;
let pendingState;

function enqueueState(state, message) {
  pendingState = { state, message, seq: nextSequence() };
  if (!sending) void flushStates();
}

async function flushStates() {
  if (sending) return;
  sending = true;
  try {
    while (pendingState) {
      const report = pendingState;
      pendingState = undefined;
      await reportState(report.state, report.message, report.seq);
    }
  } finally {
    sending = false;
    if (pendingState) void flushStates();
  }
}

export default function installHerdrLifecycle(pi) {
  if (!isEnabled()) return;

  let rootInteractiveSession = false;
  let agentWorking = false;
  let blockerDepth = 0;
  let blockerLabel;
  let lastState;
  let lastMessage;

  function desiredState() {
    if (blockerDepth > 0) return { state: "blocked", message: blockerLabel };
    return { state: agentWorking ? "working" : "idle", message: undefined };
  }

  function publish(force = false) {
    const next = desiredState();
    if (!force && next.state === lastState && next.message === lastMessage) return;
    lastState = next.state;
    lastMessage = next.message;
    enqueueState(next.state, next.message);
  }

  pi.events.on("herdr:blocked", (event) => {
    if (!rootInteractiveSession) return;

    if (event?.active) {
      blockerDepth += 1;
      blockerLabel = event.label;
    } else {
      blockerDepth = Math.max(0, blockerDepth - 1);
      if (blockerDepth === 0) blockerLabel = undefined;
    }
    publish();
  });

  pi.on("session_start", async (event, ctx) => {
    if (ctx?.hasUI !== true) return;

    rootInteractiveSession = true;
    refreshSession(ctx);
    // Session identity must reach Herdr before state for /new, /resume, /fork, and /reload.
    await reportSession(event?.reason);
    agentWorking = ctx?.isIdle?.() === false;
    publish(true);
  });

  pi.on("agent_start", (_event, ctx) => {
    if (!rootInteractiveSession) return;

    refreshSession(ctx);
    void reportSession();
    agentWorking = true;
    publish();
  });

  pi.on("agent_settled", (_event, ctx) => {
    if (!rootInteractiveSession || ctx?.isIdle?.() !== true) return;

    agentWorking = false;
    publish();
  });
}
