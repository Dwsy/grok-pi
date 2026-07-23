/**
 * grok-pi /loop — Grok-style scheduled recurring prompt (interval scheduler).
 *
 * Not the Pi until-success agentic loop. Aligns with stock Grok:
 *  - /loop [interval] <prompt>
 *  - scheduler_create / scheduler_delete / scheduler_list
 *  - process-local timers fire as followUp prompts
 *  - appendEntry bridge → adapter → native ScheduledTask* UI
 *
 * See docs/issues/adapter/20260722-grok-pi-loop.md
 */
import { randomUUID } from "node:crypto";
import { readFileSync, writeFileSync, existsSync } from "node:fs";
import { Type } from "@sinclair/typebox";
import type { ExtensionAPI, ExtensionCommandContext } from "@earendil-works/pi-coding-agent";

const BRIDGE_TYPE = "pi-grok-loop/v1";
const MIN_INTERVAL_SECS = 60;
const MAX_TASKS = 50;
const EXPIRE_MS = 7 * 24 * 60 * 60 * 1000;

type ScheduledTask = {
  id: string;
  intervalSecs: number;
  prompt: string;
  humanSchedule: string;
  createdAt: string;
  nextFireAt: string;
  fireImmediately: boolean;
  expiresAt: string;
};

type LoopControl = {
  tasks: ScheduledTask[];
};

type RuntimeTask = ScheduledTask & {
  timer?: ReturnType<typeof setInterval>;
  firing?: boolean;
};

const runtime = new Map<string, RuntimeTask>();

function controlPath(): string | undefined {
  const v = process.env.PI_GROK_LOOP_CONTROL?.trim();
  return v || undefined;
}

function readControl(): LoopControl {
  const path = controlPath();
  if (!path || !existsSync(path)) return { tasks: [] };
  try {
    const raw = JSON.parse(readFileSync(path, "utf8")) as LoopControl;
    if (!Array.isArray(raw?.tasks)) return { tasks: [] };
    return { tasks: raw.tasks };
  } catch {
    return { tasks: [] };
  }
}

function writeControl(control: LoopControl): void {
  const path = controlPath();
  if (!path) return;
  writeFileSync(path, JSON.stringify(control, null, 2), "utf8");
}

function persistRuntime(): void {
  const tasks = [...runtime.values()].map(
    ({ timer: _t, firing: _f, ...task }) => task,
  );
  writeControl({ tasks });
}

function emitBridge(
  pi: ExtensionAPI,
  event: string,
  task: ScheduledTask,
  extra?: Record<string, unknown>,
): void {
  try {
    pi.appendEntry(BRIDGE_TYPE, {
      type: event,
      task,
      ...extra,
    });
  } catch {
    // best-effort
  }
}

function parseIntervalToken(token: string): number | null {
  const t = token.trim().toLowerCase();
  if (t.length < 2) return null;
  const unit = t.slice(-1);
  const n = Number(t.slice(0, -1));
  if (!Number.isFinite(n) || n <= 0 || !Number.isInteger(n)) return null;
  switch (unit) {
    case "s":
      return n;
    case "m":
      return n * 60;
    case "h":
      return n * 3600;
    case "d":
      return n * 86400;
    default:
      return null;
  }
}

function clampInterval(secs: number): number {
  return Math.max(MIN_INTERVAL_SECS, secs);
}

function intervalToHuman(secs: number): string {
  if (secs % 86400 === 0) {
    const n = secs / 86400;
    return n === 1 ? "every 1 day" : `every ${n} days`;
  }
  if (secs % 3600 === 0) {
    const n = secs / 3600;
    return n === 1 ? "every 1 hour" : `every ${n} hours`;
  }
  if (secs % 60 === 0) {
    const n = secs / 60;
    return n === 1 ? "every 1 minute" : `every ${n} minutes`;
  }
  return secs === 1 ? "every 1 second" : `every ${secs} seconds`;
}

function parseLoopArgs(args: string): { intervalSecs: number | null; prompt: string } {
  const trimmed = args.trim();
  const space = trimmed.search(/\s/);
  if (space > 0) {
    const first = trimmed.slice(0, space);
    const rest = trimmed.slice(space).trim();
    const secs = parseIntervalToken(first);
    if (secs != null && rest) {
      return { intervalSecs: secs, prompt: rest };
    }
  }
  return { intervalSecs: null, prompt: trimmed };
}

function scheduleInstruction(args: string): string {
  return (
    `# /loop -- schedule a recurring prompt\n\n` +
    `Parse the input below into an interval and a prompt, then schedule it with scheduler_create.\n\n` +
    `## Deriving the interval\n` +
    `Read how often to run from the user's request — however they phrase it — and convert it\n` +
    `to a compact \`<number><unit>\` string, where unit is one of \`s\` (seconds), \`m\` (minutes),\n` +
    `\`h\` (hours), or \`d\` (days). The interval may appear at the start or end of the request;\n` +
    `extract it and use the remaining text as the prompt.\n\n` +
    `The minimum interval is 60 seconds; shorter values are raised to 60s, so tell the user if that applies.\n\n` +
    `If the request contains no interval at all, ask the user how often it should run before\n` +
    `scheduling. Do NOT invent or assume a default interval.\n\n` +
    `## Action\n` +
    `1. Call scheduler_create with: interval (the compact string you derived), prompt,\n` +
    `   fire_immediately: true. If the interval is unparseable, the tool\n` +
    `   returns an error — fix the interval string rather than guessing.\n` +
    `2. Confirm: what's scheduled, the cadence, that it auto-expires after 7 days,\n` +
    `   and that they can cancel with scheduler_delete (include the job ID).\n` +
    `3. Do NOT execute the prompt inline. The scheduler will fire it immediately.\n\n` +
    `## Changing an existing loop\n` +
    `Call scheduler_create with its task_id and the fields that change; do not\n` +
    `delete and recreate. If later work changes what a loop should do, update its\n` +
    `prompt the same way.\n\n` +
    `## One-time delayed work\n` +
    `Scheduling is recurring-only. For "do X once in N minutes", run a background\n` +
    `terminal command (\`sleep <secs> && <command>\`); its completion notifies you.\n\n` +
    `## Input\n` +
    `${args}`
  );
}

function firePrompt(pi: ExtensionAPI, task: ScheduledTask): string {
  return (
    `<scheduled-task task_id="${task.id}" schedule="${task.humanSchedule}">\n` +
    `${task.prompt}\n` +
    `</scheduled-task>`
  );
}

function armTimer(pi: ExtensionAPI, task: RuntimeTask): void {
  if (task.timer) {
    clearInterval(task.timer);
    task.timer = undefined;
  }
  const ms = task.intervalSecs * 1000;
  task.timer = setInterval(() => {
    void onFire(pi, task.id);
  }, ms);
  // Prevent keeping process alive solely for timers in some hosts.
  if (typeof task.timer === "object" && task.timer && "unref" in task.timer) {
    try {
      (task.timer as NodeJS.Timeout).unref?.();
    } catch {
      // ignore
    }
  }
}

async function onFire(pi: ExtensionAPI, taskId: string): Promise<void> {
  const task = runtime.get(taskId);
  if (!task) return;
  if (Date.now() >= Date.parse(task.expiresAt)) {
    removeTask(pi, taskId, "expired");
    return;
  }
  if (task.firing) {
    // Skip overlapping fire (upstream skips while previous iteration runs).
    return;
  }
  task.firing = true;
  const next = new Date(Date.now() + task.intervalSecs * 1000).toISOString();
  task.nextFireAt = next;
  persistRuntime();
  emitBridge(pi, "scheduled_task_fired", task, { nextFireAt: next });
  try {
    pi.sendUserMessage(firePrompt(pi, task), { deliverAs: "followUp" });
  } catch {
    // best-effort
  } finally {
    task.firing = false;
  }
}

function upsertTask(
  pi: ExtensionAPI,
  input: {
    taskId?: string;
    intervalSecs: number;
    prompt: string;
    fireImmediately: boolean;
  },
): ScheduledTask {
  const now = Date.now();
  const intervalSecs = clampInterval(input.intervalSecs);
  const humanSchedule = intervalToHuman(intervalSecs);
  const nextFireAt = new Date(
    now + (input.fireImmediately ? 0 : intervalSecs * 1000),
  ).toISOString();

  if (input.taskId) {
    const existing = runtime.get(input.taskId);
    if (!existing) {
      throw new Error(`Unknown task_id: ${input.taskId}`);
    }
    existing.intervalSecs = intervalSecs;
    existing.prompt = input.prompt;
    existing.humanSchedule = humanSchedule;
    existing.nextFireAt = nextFireAt;
    existing.fireImmediately = input.fireImmediately;
    armTimer(pi, existing);
    persistRuntime();
    emitBridge(pi, "scheduled_task_created", existing);
    if (input.fireImmediately) {
      void onFire(pi, existing.id);
    }
    return existing;
  }

  if (runtime.size >= MAX_TASKS) {
    throw new Error(`Maximum ${MAX_TASKS} scheduled tasks`);
  }

  const task: RuntimeTask = {
    id: randomUUID(),
    intervalSecs,
    prompt: input.prompt,
    humanSchedule,
    createdAt: new Date(now).toISOString(),
    nextFireAt,
    fireImmediately: input.fireImmediately,
    expiresAt: new Date(now + EXPIRE_MS).toISOString(),
  };
  runtime.set(task.id, task);
  armTimer(pi, task);
  persistRuntime();
  emitBridge(pi, "scheduled_task_created", task);
  if (input.fireImmediately) {
    void onFire(pi, task.id);
  }
  return task;
}

function removeTask(pi: ExtensionAPI, taskId: string, reason: string): boolean {
  const task = runtime.get(taskId);
  if (!task) return false;
  if (task.timer) clearInterval(task.timer);
  runtime.delete(taskId);
  persistRuntime();
  emitBridge(pi, "scheduled_task_deleted", task, { reason });
  return true;
}

function hydrate(pi: ExtensionAPI): void {
  const control = readControl();
  for (const t of control.tasks) {
    if (Date.now() >= Date.parse(t.expiresAt)) continue;
    const rt: RuntimeTask = { ...t };
    runtime.set(rt.id, rt);
    armTimer(pi, rt);
    emitBridge(pi, "scheduled_task_created", rt);
  }
}

export default function piGrokLoop(pi: ExtensionAPI) {
  if (process.env.PI_GROK !== "1") return;
  if (!controlPath()) return;

  hydrate(pi);

  pi.registerCommand("loop", {
    description: "Run a prompt on a recurring interval",
    handler: async (args: string, ctx: ExtensionCommandContext) => {
      const trimmed = args.trim();
      if (!trimmed || trimmed === "help") {
        ctx.ui.notify(
          "Usage: /loop [interval] <prompt>\nExample: /loop 30m check deploy status\nAlso: /loop list | /loop stop <id>|all",
          "info",
        );
        return;
      }

      const sub = trimmed.split(/\s+/)[0]?.toLowerCase() ?? "";
      if (sub === "list") {
        if (runtime.size === 0) {
          ctx.ui.notify("No scheduled loops.", "info");
          return;
        }
        const lines = [...runtime.values()].map(
          (t) =>
            `${t.id.slice(0, 8)} · ${t.humanSchedule} · next ${t.nextFireAt}\n  ${t.prompt}`,
        );
        ctx.ui.notify(lines.join("\n"), "info");
        return;
      }
      if (sub === "stop" || sub === "cancel") {
        const id = trimmed.slice(sub.length).trim();
        if (!id || id === "all") {
          const ids = [...runtime.keys()];
          for (const taskId of ids) removeTask(pi, taskId, "user");
          ctx.ui.notify(
            ids.length ? `Stopped ${ids.length} loop(s).` : "No loops to stop.",
            "info",
          );
          return;
        }
        // Match by full id or prefix.
        const match =
          runtime.get(id) ??
          [...runtime.values()].find((t) => t.id.startsWith(id));
        if (!match) {
          ctx.ui.notify(`No loop matching ${id}`, "warning");
          return;
        }
        removeTask(pi, match.id, "user");
        ctx.ui.notify(`Stopped loop ${match.id.slice(0, 8)}.`, "info");
        return;
      }

      const { intervalSecs, prompt } = parseLoopArgs(trimmed);
      if (intervalSecs != null && prompt) {
        try {
          const task = upsertTask(pi, {
            intervalSecs,
            prompt,
            fireImmediately: true,
          });
          const raised =
            intervalSecs < MIN_INTERVAL_SECS
              ? ` (raised to ${MIN_INTERVAL_SECS}s minimum)`
              : "";
          ctx.ui.notify(
            `Scheduled ${task.humanSchedule}${raised}. id=${task.id.slice(0, 8)} (expires 7d). Cancel: /loop stop ${task.id.slice(0, 8)}`,
            "info",
          );
        } catch (e) {
          ctx.ui.notify(String(e), "error");
        }
        return;
      }

      // Natural language / no leading interval — model derives via tool.
      pi.sendUserMessage(scheduleInstruction(trimmed));
    },
  });

  pi.registerTool({
    name: "scheduler_create",
    label: "Scheduler Create",
    description:
      "Create a scheduled task that runs a prompt on a recurring interval, or update an existing one in place. Interval: 5m/2h/1d/60s (min 60s). fire_immediately defaults false for the tool; /loop uses true.",
    parameters: Type.Object({
      task_id: Type.Optional(
        Type.String({
          description: "Existing task id to update in place.",
        }),
      ),
      interval: Type.Optional(
        Type.String({
          description: 'Interval e.g. "5m", "2h", "1d", "60s". Required to create.',
        }),
      ),
      prompt: Type.Optional(
        Type.String({
          description: "Prompt text for each fire. Required to create.",
        }),
      ),
      fire_immediately: Type.Optional(
        Type.Boolean({
          description:
            "Fire once on create/update (true) or wait for first interval (false). Default false.",
        }),
      ),
    }),
    async execute(
      _toolCallId: string,
      params: {
        task_id?: string;
        interval?: string;
        prompt?: string;
        fire_immediately?: boolean;
      },
    ) {
      try {
        if (params.task_id) {
          const existing = runtime.get(params.task_id);
          if (!existing) {
            return {
              content: [{ type: "text", text: `Unknown task_id: ${params.task_id}` }],
              details: { ok: false },
            };
          }
          const secs = params.interval
            ? parseIntervalToken(params.interval)
            : existing.intervalSecs;
          if (secs == null) {
            return {
              content: [{ type: "text", text: `Invalid interval: ${params.interval}` }],
              details: { ok: false },
            };
          }
          const task = upsertTask(pi, {
            taskId: params.task_id,
            intervalSecs: secs,
            prompt: params.prompt?.trim() || existing.prompt,
            fireImmediately: params.fire_immediately === true,
          });
          return {
            content: [
              {
                type: "text",
                text: `Updated task ${task.id} (${task.humanSchedule}).`,
              },
            ],
            details: {
              id: task.id,
              humanSchedule: task.humanSchedule,
              updated: true,
            },
          };
        }

        const interval = params.interval?.trim();
        const prompt = params.prompt?.trim();
        if (!interval || !prompt) {
          return {
            content: [
              {
                type: "text",
                text: "interval and prompt are required to create a task.",
              },
            ],
            details: { ok: false },
          };
        }
        const secs = parseIntervalToken(interval);
        if (secs == null) {
          return {
            content: [{ type: "text", text: `Invalid interval: ${interval}` }],
            details: { ok: false },
          };
        }
        const task = upsertTask(pi, {
          intervalSecs: secs,
          prompt,
          fireImmediately: params.fire_immediately === true,
        });
        return {
          content: [
            {
              type: "text",
              text: `Scheduled ${task.humanSchedule}. id=${task.id}. Auto-expires after 7 days. Cancel with scheduler_delete.`,
            },
          ],
          details: {
            id: task.id,
            humanSchedule: task.humanSchedule,
            updated: false,
          },
        };
      } catch (e) {
        return {
          content: [{ type: "text", text: String(e) }],
          details: { ok: false },
        };
      }
    },
  });

  pi.registerTool({
    name: "scheduler_delete",
    label: "Scheduler Delete",
    description: "Cancel a scheduled task by task_id (from scheduler_create).",
    parameters: Type.Object({
      task_id: Type.String({ description: "Task ID to cancel" }),
    }),
    async execute(_toolCallId: string, params: { task_id: string }) {
      const ok = removeTask(pi, params.task_id, "delete");
      return {
        content: [
          {
            type: "text",
            text: ok
              ? `Deleted scheduled task ${params.task_id}`
              : `No task ${params.task_id}`,
          },
        ],
        details: { ok, task_id: params.task_id },
      };
    },
  });

  pi.registerTool({
    name: "scheduler_list",
    label: "Scheduler List",
    description: "List active scheduled tasks for this session.",
    parameters: Type.Object({}),
    async execute() {
      const tasks = [...runtime.values()].map((t) => ({
        id: t.id,
        human_schedule: t.humanSchedule,
        prompt: t.prompt,
        next_fire_at: t.nextFireAt,
      }));
      return {
        content: [
          {
            type: "text",
            text:
              tasks.length === 0
                ? "No scheduled tasks."
                : tasks
                    .map(
                      (t) =>
                        `${t.id} · ${t.human_schedule} · next ${t.next_fire_at}\n  ${t.prompt}`,
                    )
                    .join("\n"),
          },
        ],
        details: { tasks },
      };
    },
  });
}
