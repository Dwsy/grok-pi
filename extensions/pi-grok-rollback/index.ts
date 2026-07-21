/**
 * Pi Tree File Rollback — checkpoint extension.
 *
 * Wraps Pi builtin write/edit tools via their public Operations seam to
 * capture before/after byte snapshots inside the mutation queue critical
 * section. Maintains a durable WAL journal with content-addressed blob
 * storage, binds mutations to session tree entries, and serves
 * preview/execute requests from the grok-pi adapter via a control-directory
 * bridge.
 *
 * Injected only when F2 "Pi tree file rollback" is enabled.
 * Does NOT modify Pi source.
 */

import { AsyncLocalStorage } from "node:async_hooks";
import { createHash, randomUUID } from "node:crypto";
import { constants } from "node:fs";
import {
  access as fsAccess,
  appendFile,
  chmod,
  lstat,
  mkdir as fsMkdir,
  readFile as fsReadFile,
  readdir,
  rename,
  rm,
  stat,
  unlink,
  writeFile as fsWriteFile,
} from "node:fs/promises";
import { dirname, isAbsolute, join, resolve } from "node:path";

import {
  createEditToolDefinition,
  createWriteToolDefinition,
} from "@earendil-works/pi-coding-agent";

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const JOURNAL_VERSION = 1;
const BRIDGE_VERSION = 1;
const MAX_BLOB_SIZE = 10 * 1024 * 1024; // 10 MB per blob
const STALE_BRIDGE_MS = 60_000;
const BRIDGE_POLL_MS = 50;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

interface JournalHeader {
  version: number;
  piSessionId: string;
  canonicalSessionFile?: string;
  sessionHeaderDigest?: string;
  origin: "new" | "continued" | "resumed" | "forked" | "ephemeral";
  captureBoundaryEntryId?: string;
  createdByGrokPi: true;
}

interface MutationRecord {
  sequence: number;
  operationId: string;
  piSessionId: string;
  treeEntryId?: string;
  toolCallId: string;
  tool: "write" | "edit";
  canonicalPath: string;
  before: "absent" | string; // "absent" or "blob:<sha256hex>"
  after: "absent" | string;
  state: "prepared" | "committed" | "unbound" | "reconciled";
  toolReportedError: boolean;
  preparedAt: string;
  committedAt?: string;
}

interface RollbackTransaction {
  transactionId: string;
  targetEntryId: string;
  sourceLeafId: string;
  plannedPaths: string[];
  state: "prepared" | "committed" | "compensating" | "failed";
  createdAt: string;
}

interface BridgeRequest {
  version: number;
  nonce: string;
  sessionId: string;
  method: "preview" | "execute";
  params: { targetEntryId: string };
  createdAt: string;
}

interface BridgeResponse {
  version: number;
  nonce: string;
  sessionId: string;
  method: "preview" | "execute";
  ok: boolean;
  result?: {
    eligible: boolean;
    paths: Array<{
      canonicalPath: string;
      action: "restore" | "delete" | "noop";
      currentDigest: string | null;
      targetDigest: string | null;
    }>;
    conflicts: string[];
    transactionId?: string;
  };
  error?: string;
  completedAt: string;
}

// ---------------------------------------------------------------------------
// AsyncLocalStorage for toolCallId propagation into operations
// ---------------------------------------------------------------------------

interface ToolCallContext {
  toolCallId: string;
  tool: "write" | "edit";
}

const toolCallStorage = new AsyncLocalStorage<ToolCallContext>();

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

let stateRoot = "";
let controlDir = "";
let sessionId = "";
let sessionDir = "";
let blobDir = "";
let journalPath = "";
let headerPath = "";
let sequence = 0;
let active = false;
let extensionCwd = "";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function sha256(buf: Buffer): string {
  return createHash("sha256").update(buf).digest("hex");
}

function blobRef(hash: string): string {
  return `blob:${hash}`;
}

function parseBlobRef(ref: string): string | null {
  if (ref === "absent") return null;
  if (ref.startsWith("blob:")) return ref.slice(5);
  return null;
}

async function ensureDir(dir: string): Promise<void> {
  await fsMkdir(dir, { recursive: true });
  await chmod(dir, 0o700).catch(() => {});
}

async function writeSecure(path: string, data: Buffer | string): Promise<void> {
  await fsWriteFile(path, data);
  await chmod(path, 0o600).catch(() => {});
}

async function appendJournal(record: MutationRecord | RollbackTransaction): Promise<void> {
  const line = JSON.stringify(record) + "\n";
  await appendFile(journalPath, line, "utf-8");
}

async function readBlob(hash: string): Promise<Buffer | null> {
  try {
    return await fsReadFile(join(blobDir, hash));
  } catch {
    return null;
  }
}

async function writeBlob(data: Buffer): Promise<string> {
  if (data.length > MAX_BLOB_SIZE) {
    throw new Error(`blob exceeds ${MAX_BLOB_SIZE} byte limit (${data.length})`);
  }
  const hash = sha256(data);
  const p = join(blobDir, hash);
  try {
    await stat(p);
    return hash; // dedup
  } catch {
    // not found
  }
  const tmp = `${p}.tmp.${randomUUID()}`;
  await fsWriteFile(tmp, data);
  await chmod(tmp, 0o600).catch(() => {});
  await rename(tmp, p);
  return hash;
}

async function isSymlink(path: string): Promise<boolean> {
  try {
    return (await lstat(path)).isSymbolicLink();
  } catch {
    return false;
  }
}

function canonicalize(filePath: string): string {
  return isAbsolute(filePath) ? resolve(filePath) : resolve(extensionCwd, filePath);
}

async function readJournalRecords(): Promise<Array<MutationRecord | RollbackTransaction>> {
  try {
    const content = await fsReadFile(journalPath, "utf-8");
    return content.split("\n").filter((l) => l.trim()).map((l) => JSON.parse(l));
  } catch {
    return [];
  }
}

function getMutations(records: Array<MutationRecord | RollbackTransaction>): MutationRecord[] {
  return records.filter((r): r is MutationRecord => "operationId" in r);
}

function getTransactions(records: Array<MutationRecord | RollbackTransaction>): RollbackTransaction[] {
  return records.filter((r): r is RollbackTransaction => "transactionId" in r);
}

// ---------------------------------------------------------------------------
// Journal initialization
// ---------------------------------------------------------------------------

async function initJournal(sid: string, origin: JournalHeader["origin"], boundaryEntryId?: string): Promise<void> {
  sessionId = sid;
  sessionDir = join(stateRoot, "sessions", sid);
  blobDir = join(stateRoot, "blobs");
  journalPath = join(sessionDir, "journal.jsonl");
  headerPath = join(sessionDir, "header.json");

  await ensureDir(sessionDir);
  await ensureDir(blobDir);

  let existingHeader: JournalHeader | null = null;
  try {
    existingHeader = JSON.parse(await fsReadFile(headerPath, "utf-8"));
  } catch {
    // no existing header
  }

  if (existingHeader && existingHeader.piSessionId === sid) {
    // Resume existing journal
    const records = await readJournalRecords();
    const mutations = getMutations(records);
    sequence = mutations.reduce((max, m) => Math.max(max, m.sequence), 0);
    return;
  }

  const header: JournalHeader = {
    version: JOURNAL_VERSION,
    piSessionId: sid,
    origin,
    captureBoundaryEntryId: boundaryEntryId,
    createdByGrokPi: true,
  };
  await writeSecure(headerPath, JSON.stringify(header, null, 2));
  await writeSecure(journalPath, "");
  sequence = 0;
}

// ---------------------------------------------------------------------------
// Mutation capture — called from within the mutation queue critical section
// ---------------------------------------------------------------------------

async function captureMutation(
  tool: "write" | "edit",
  toolCallId: string,
  canonicalPath: string,
  beforeData: Buffer | null,
  afterData: Buffer | null,
  toolReportedError: boolean,
): Promise<void> {
  if (!active) return;

  // Reject symlinks at final target
  if (await isSymlink(canonicalPath)) {
    throw new Error(`rollback: refusing to checkpoint symlink target: ${canonicalPath}`);
  }

  let before: string;
  if (beforeData === null) {
    before = "absent";
  } else {
    before = blobRef(await writeBlob(beforeData));
  }

  let after: string;
  if (afterData === null) {
    after = "absent";
  } else {
    after = blobRef(await writeBlob(afterData));
  }

  // Skip if nothing changed
  if (before === after) return;

  sequence++;
  const record: MutationRecord = {
    sequence,
    operationId: randomUUID(),
    piSessionId: sessionId,
    toolCallId,
    tool,
    canonicalPath,
    before,
    after,
    state: "unbound",
    toolReportedError,
    preparedAt: new Date().toISOString(),
  };

  await appendJournal(record);
}

// ---------------------------------------------------------------------------
// Tree entry binding
// ---------------------------------------------------------------------------

async function bindTreeEntries(ctx: any): Promise<void> {
  if (!active) return;
  try {
    const entries = ctx.sessionManager?.getEntries?.();
    if (!entries) return;

    const records = await readJournalRecords();
    const unbound = getMutations(records).filter((m) => m.state === "unbound");
    if (unbound.length === 0) return;

    const toolResultMap = new Map<string, string>();
    for (const entry of entries) {
      if (entry.type === "toolResult" && entry.toolCallId) {
        toolResultMap.set(entry.toolCallId, entry.id);
      }
    }

    let updated = false;
    for (const m of unbound) {
      const entryId = toolResultMap.get(m.toolCallId);
      if (entryId) {
        m.treeEntryId = entryId;
        m.state = "reconciled";
        updated = true;
      }
    }

    if (updated) {
      const allRecords = await readJournalRecords();
      const mutations = getMutations(allRecords).map((m) => {
        const bound = unbound.find((u) => u.operationId === m.operationId);
        return bound ?? m;
      });
      const transactions = getTransactions(allRecords);
      const all = [...mutations, ...transactions].sort(
        (a, b) => ("sequence" in a ? a.sequence : 0) - ("sequence" in b ? b.sequence : 0),
      );
      await writeSecure(journalPath, all.map((r) => JSON.stringify(r)).join("\n") + "\n");
    }
  } catch {
    // Best-effort
  }
}

// ---------------------------------------------------------------------------
// Rollback logic
// ---------------------------------------------------------------------------

interface RollbackPlan {
  eligible: boolean;
  paths: Array<{
    canonicalPath: string;
    action: "restore" | "delete" | "noop";
    currentDigest: string | null;
    targetDigest: string | null;
  }>;
  conflicts: string[];
}

function getBranchEntryIds(entries: any[]): Set<string> {
  const ids = new Set<string>();
  let current = entries.find((e: any) => e.isCurrent || e.isLeaf);
  while (current) {
    ids.add(current.id);
    current = entries.find((e: any) => e.id === current.parentId);
  }
  return ids;
}

async function computeRollbackPlan(
  targetEntryId: string,
  branchEntryIds: Set<string>,
): Promise<RollbackPlan> {
  const records = await readJournalRecords();
  const mutations = getMutations(records);

  if (!branchEntryIds.has(targetEntryId)) {
    return { eligible: false, paths: [], conflicts: [`target entry ${targetEntryId} not on active branch`] };
  }

  // Mutations whose treeEntryId is a strict descendant of target on this branch
  const afterTarget = mutations.filter((m) => {
    if (!m.treeEntryId) return false;
    return branchEntryIds.has(m.treeEntryId) && m.treeEntryId !== targetEntryId;
  });

  // Group by canonicalPath: first mutation gives target state (before), last gives expected current (after)
  const pathFirst = new Map<string, MutationRecord>();
  const pathLast = new Map<string, MutationRecord>();
  for (const m of afterTarget) {
    if (!pathFirst.has(m.canonicalPath)) pathFirst.set(m.canonicalPath, m);
    pathLast.set(m.canonicalPath, m);
  }

  const paths: RollbackPlan["paths"] = [];
  const conflicts: string[] = [];

  for (const [canonicalPath, lastMutation] of pathLast) {
    const firstMutation = pathFirst.get(canonicalPath)!;
    const targetState = firstMutation.before;
    const expectedCurrentState = lastMutation.after;

    // Read current file
    let currentDigest: string | null = null;
    let currentExists = false;
    try {
      const data = await fsReadFile(canonicalPath);
      currentExists = true;
      currentDigest = sha256(data);
    } catch {
      currentExists = false;
    }

    // Verify current state matches expected
    const expectedHash = parseBlobRef(expectedCurrentState);
    if (expectedCurrentState === "absent") {
      if (currentExists) {
        conflicts.push(`${canonicalPath}: expected absent but file exists (external modification)`);
        continue;
      }
      paths.push({ canonicalPath, action: "noop", currentDigest: null, targetDigest: null });
    } else if (expectedHash) {
      if (!currentExists) {
        conflicts.push(`${canonicalPath}: expected content but file missing (external deletion)`);
        continue;
      }
      if (currentDigest !== expectedHash) {
        conflicts.push(`${canonicalPath}: content mismatch (external modification)`);
        continue;
      }
    }

    // Determine action
    const targetHash = parseBlobRef(targetState);
    if (targetState === "absent") {
      paths.push({
        canonicalPath,
        action: currentExists ? "delete" : "noop",
        currentDigest,
        targetDigest: null,
      });
    } else if (targetHash) {
      if (currentExists && currentDigest === targetHash) {
        paths.push({ canonicalPath, action: "noop", currentDigest, targetDigest: targetHash });
      } else {
        paths.push({ canonicalPath, action: "restore", currentDigest, targetDigest: targetHash });
      }
    }
  }

  return {
    eligible: conflicts.length === 0 && paths.some((p) => p.action !== "noop"),
    paths,
    conflicts,
  };
}

async function executeRollback(
  plan: RollbackPlan,
  targetEntryId: string,
  sourceLeafId: string,
): Promise<string> {
  const transactionId = randomUUID();
  const activePaths = plan.paths.filter((p) => p.action !== "noop");

  const tx: RollbackTransaction = {
    transactionId,
    targetEntryId,
    sourceLeafId,
    plannedPaths: activePaths.map((p) => p.canonicalPath),
    state: "prepared",
    createdAt: new Date().toISOString(),
  };
  await appendJournal(tx);

  try {
    for (const p of activePaths) {
      if (p.action === "delete") {
        await unlink(p.canonicalPath).catch(() => {});
      } else if (p.action === "restore" && p.targetDigest) {
        const data = await readBlob(p.targetDigest);
        if (!data) throw new Error(`missing blob for ${p.canonicalPath}`);
        const tmp = `${p.canonicalPath}.rollback-tmp.${randomUUID()}`;
        await fsWriteFile(tmp, data);
        await rename(tmp, p.canonicalPath);
      }
    }
    tx.state = "committed";
    await appendJournal(tx);
  } catch (err) {
    tx.state = "failed";
    await appendJournal(tx);
    throw err;
  }

  return transactionId;
}

// ---------------------------------------------------------------------------
// Bridge: control-directory request/response
// ---------------------------------------------------------------------------

async function cleanStaleBridgeFiles(): Promise<void> {
  try {
    const files = await readdir(controlDir);
    const now = Date.now();
    for (const f of files) {
      const p = join(controlDir, f);
      try {
        const s = await stat(p);
        if (now - s.mtimeMs > STALE_BRIDGE_MS) await rm(p, { force: true });
      } catch { /* ignore */ }
    }
  } catch { /* control dir may not exist */ }
}

async function processBridgeRequest(req: BridgeRequest, ctx: any): Promise<void> {
  const responsePath = join(controlDir, `response-${req.nonce}.json`);
  const tmpPath = `${responsePath}.tmp.${randomUUID()}`;

  let response: BridgeResponse;
  try {
    if (req.version !== BRIDGE_VERSION) throw new Error(`unsupported bridge version: ${req.version}`);
    if (req.sessionId !== sessionId) throw new Error(`session mismatch: ${req.sessionId} !== ${sessionId}`);

    const entries = ctx.sessionManager?.getEntries?.() ?? [];
    const branchEntryIds = getBranchEntryIds(entries);

    if (req.method === "preview") {
      const plan = await computeRollbackPlan(req.params.targetEntryId, branchEntryIds);
      response = {
        version: BRIDGE_VERSION, nonce: req.nonce, sessionId, method: "preview", ok: true,
        result: { eligible: plan.eligible, paths: plan.paths, conflicts: plan.conflicts },
        completedAt: new Date().toISOString(),
      };
    } else if (req.method === "execute") {
      const plan = await computeRollbackPlan(req.params.targetEntryId, branchEntryIds);
      if (!plan.eligible) {
        response = {
          version: BRIDGE_VERSION, nonce: req.nonce, sessionId, method: "execute", ok: false,
          error: plan.conflicts.join("; ") || "no eligible paths",
          completedAt: new Date().toISOString(),
        };
      } else {
        const leafId = entries.find((e: any) => e.isCurrent || e.isLeaf)?.id ?? "unknown";
        const txId = await executeRollback(plan, req.params.targetEntryId, leafId);
        response = {
          version: BRIDGE_VERSION, nonce: req.nonce, sessionId, method: "execute", ok: true,
          result: { eligible: true, paths: plan.paths, conflicts: [], transactionId: txId },
          completedAt: new Date().toISOString(),
        };
      }
    } else {
      throw new Error(`unknown method: ${req.method}`);
    }
  } catch (err: any) {
    response = {
      version: BRIDGE_VERSION, nonce: req.nonce, sessionId, method: req.method, ok: false,
      error: err?.message ?? String(err),
      completedAt: new Date().toISOString(),
    };
  }

  await fsWriteFile(tmpPath, JSON.stringify(response, null, 2));
  await chmod(tmpPath, 0o600).catch(() => {});
  await rename(tmpPath, responsePath);
}

async function pollBridgeRequests(ctx: any): Promise<void> {
  if (!controlDir) return;
  try {
    const files = await readdir(controlDir);
    for (const f of files) {
      if (!f.startsWith("request-") || !f.endsWith(".json")) continue;
      const reqPath = join(controlDir, f);
      try {
        const raw = await fsReadFile(reqPath, "utf-8");
        const req: BridgeRequest = JSON.parse(raw);
        await rm(reqPath, { force: true });
        await processBridgeRequest(req, ctx);
      } catch { /* skip malformed */ }
    }
  } catch { /* control dir may not exist */ }
}

// ---------------------------------------------------------------------------
// Extension entry point
// ---------------------------------------------------------------------------

export default function (pi: any) {
  const enabled = process.env.PI_GROK_ROLLBACK === "1";
  if (!enabled) return;

  stateRoot = process.env.GROK_PI_ROLLBACK_STATE || join(process.env.HOME || "/tmp", ".grok", "pi-file-rollback");
  controlDir = process.env.GROK_PI_ROLLBACK_CONTROL || "";
  extensionCwd = process.cwd();

  let bridgeCtx: any = null;
  let bridgeTimer: ReturnType<typeof setInterval> | null = null;

  pi.on("session_start", async (_event: any, ctx: any) => {
    extensionCwd = ctx.cwd || process.cwd();
    const sid = ctx.sessionManager?.sessionId || ctx.sessionManager?.id || "unknown";
    const origin: JournalHeader["origin"] =
      process.env.PI_GROK_ROLLBACK_ORIGIN === "resumed" ? "resumed" : "new";

    await initJournal(sid, origin);
    active = true;

    // Verify write/edit are still Pi builtin (not overridden by user extension)
    const allTools = pi.getAllTools?.() ?? [];
    for (const name of ["write", "edit"]) {
      const info = allTools.find((t: any) => t.name === name);
      if (info && info.source && info.source !== "builtin" && info.source !== "pi") {
        active = false;
        return;
      }
    }

    // --- Write tool wrapper ---
    // Custom operations that capture before/after inside the mutation queue.
    const writeDef = createWriteToolDefinition(extensionCwd, {
      operations: {
        mkdir: (dir: string) => fsMkdir(dir, { recursive: true }).then(() => {}),
        writeFile: async (absolutePath: string, content: string) => {
          const tc = toolCallStorage.getStore();
          // Capture before
          let beforeData: Buffer | null = null;
          try { beforeData = await fsReadFile(absolutePath); } catch { /* absent */ }

          // Actual write
          await fsWriteFile(absolutePath, content, "utf-8");

          // Capture after and record mutation
          if (tc && active) {
            const afterData = Buffer.from(content, "utf-8");
            await captureMutation("write", tc.toolCallId, absolutePath, beforeData, afterData, false).catch(() => {});
          }
        },
      },
    });

    // Wrap execute to inject toolCallId via AsyncLocalStorage
    const origWriteExecute = writeDef.execute.bind(writeDef);
    writeDef.execute = async (toolCallId: string, params: any, signal: any, onUpdate: any, toolCtx: any) => {
      return toolCallStorage.run({ toolCallId, tool: "write" }, () =>
        origWriteExecute(toolCallId, params, signal, onUpdate, toolCtx),
      );
    };
    pi.registerTool(writeDef);

    // --- Edit tool wrapper ---
    let editBeforeData: Buffer | null = null;
    let editBeforePath: string | null = null;

    const editDef = createEditToolDefinition(extensionCwd, {
      operations: {
        access: (absolutePath: string) => fsAccess(absolutePath, constants.R_OK | constants.W_OK),
        readFile: async (absolutePath: string) => {
          const data = await fsReadFile(absolutePath);
          // Store before snapshot
          editBeforeData = data;
          editBeforePath = absolutePath;
          return data;
        },
        writeFile: async (absolutePath: string, content: string) => {
          const tc = toolCallStorage.getStore();

          // Actual write
          await fsWriteFile(absolutePath, content, "utf-8");

          // Capture mutation
          if (tc && active) {
            const before = editBeforePath === absolutePath ? editBeforeData : null;
            const afterData = Buffer.from(content, "utf-8");
            await captureMutation("edit", tc.toolCallId, absolutePath, before, afterData, false).catch(() => {});
          }
          editBeforeData = null;
          editBeforePath = null;
        },
      },
    });

    const origEditExecute = editDef.execute.bind(editDef);
    editDef.execute = async (toolCallId: string, params: any, signal: any, onUpdate: any, toolCtx: any) => {
      return toolCallStorage.run({ toolCallId, tool: "edit" }, () =>
        origEditExecute(toolCallId, params, signal, onUpdate, toolCtx),
      );
    };
    pi.registerTool(editDef);

    // Start bridge polling
    if (controlDir) {
      await ensureDir(controlDir);
      await cleanStaleBridgeFiles();
      bridgeCtx = ctx;
      bridgeTimer = setInterval(() => pollBridgeRequests(bridgeCtx), BRIDGE_POLL_MS);
    }
  });

  // Bind tree entries after each turn
  pi.on("turn_end", async (_event: any, ctx: any) => {
    await bindTreeEntries(ctx);
  });

  pi.on("agent_settled", async (_event: any, ctx: any) => {
    await bindTreeEntries(ctx);
  });

  // Cleanup on shutdown
  pi.on("session_shutdown", async () => {
    active = false;
    if (bridgeTimer) {
      clearInterval(bridgeTimer);
      bridgeTimer = null;
    }
  });

  // Hidden bridge commands for adapter
  pi.registerCommand("__pi_rollback_preview", {
    description: "Internal: preview file rollback to a tree entry",
    handler: async (args: string, ctx: any) => {
      const targetEntryId = String(args ?? "").trim();
      if (!targetEntryId) throw new Error("target entry id required");
      const entries = ctx.sessionManager?.getEntries?.() ?? [];
      const branchEntryIds = getBranchEntryIds(entries);
      const plan = await computeRollbackPlan(targetEntryId, branchEntryIds);
      ctx.ui?.toast?.(
        plan.eligible
          ? `Rollback preview: ${plan.paths.filter((p) => p.action !== "noop").length} file(s) to restore`
          : `Rollback blocked: ${plan.conflicts.join("; ") || "no eligible paths"}`,
      );
    },
  });

  pi.registerCommand("__pi_rollback_execute", {
    description: "Internal: execute file rollback to a tree entry",
    handler: async (args: string, ctx: any) => {
      const targetEntryId = String(args ?? "").trim();
      if (!targetEntryId) throw new Error("target entry id required");
      if (!ctx.isIdle?.()) throw new Error("rollback requires idle session");

      const entries = ctx.sessionManager?.getEntries?.() ?? [];
      const branchEntryIds = getBranchEntryIds(entries);
      const plan = await computeRollbackPlan(targetEntryId, branchEntryIds);
      if (!plan.eligible) {
        throw new Error(`rollback blocked: ${plan.conflicts.join("; ") || "no eligible paths"}`);
      }

      const leafId = entries.find((e: any) => e.isCurrent || e.isLeaf)?.id ?? "unknown";
      const txId = await executeRollback(plan, targetEntryId, leafId);
      const count = plan.paths.filter((p) => p.action !== "noop").length;
      ctx.ui?.toast?.(`Rolled back ${count} file(s) (tx: ${txId.slice(0, 8)})`);
    },
  });
}
