/**
 * Pi Bash enhancement used only by grok-pi.
 *
 * The extension owns every Bash child process. This lets the native Pager
 * promote an active foreground tool call into its existing background-task UI
 * without rerunning the command. Pager still owns all visible task surfaces.
 */
import { randomUUID } from "node:crypto";
import { spawn, type ChildProcess } from "node:child_process";
import {
	closeSync,
	createWriteStream,
	existsSync,
	openSync,
	readFileSync,
	unlinkSync,
	watch,
	writeFileSync,
	type FSWatcher,
	type WriteStream,
} from "node:fs";
import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Type } from "@sinclair/typebox";
import { StringEnum } from "@earendil-works/pi-ai";
import {
	createBashToolDefinition,
	type ExtensionAPI,
	type ExtensionContext,
} from "@earendil-works/pi-coding-agent";

const BRIDGE_TYPE = "pi-grok-background-bash/v1";
const MAX_OUTPUT_BYTES = 50 * 1024;
const MAX_TASK_IDS = 20;
const MAX_TIMEOUT_SECONDS = 2_147_483.647;

type BashParams = {
	command: string;
	timeout?: number;
	is_background?: boolean;
	description?: string;
};

type BackgroundTask = {
	taskId: string;
	toolCallId: string;
	command: string;
	description?: string;
	cwd: string;
	outputFile: string;
	startedAt: number;
	endedAt?: number;
	child: ChildProcess;
	log: WriteStream;
	output: Buffer;
	outputBytes: number;
	truncated: boolean;
	exitCode?: number;
	signal?: string;
	completed: boolean;
	backgrounded: boolean;
	explicitlyKilled: boolean;
	timedOut: boolean;
	timeoutHandle?: ReturnType<typeof setTimeout>;
	waiters: Set<() => void>;
	foregroundSettler?: (outcome: "completed" | "backgrounded") => void;
	promote?: () => void;
	stateChanged?: () => void;
};

type BashControl = {
	sync: () => void;
	close: () => void;
};

function taskState(task: BackgroundTask): string {
	if (!task.completed) return "running";
	if (task.explicitlyKilled) return "cancelled";
	return task.exitCode === 0 && !task.signal ? "completed" : "failed";
}

function taskSnapshot(task: BackgroundTask) {
	return {
		task_id: task.taskId,
		command: task.command,
		display_command: task.command,
		cwd: task.cwd,
		start_time: systemTime(task.startedAt),
		end_time: task.endedAt === undefined ? undefined : systemTime(task.endedAt),
		output: task.output.toString("utf8"),
		output_file: task.outputFile,
		truncated: task.truncated,
		exit_code: task.exitCode,
		signal: task.signal,
		completed: task.completed,
		kind: "bash",
		block_waited: false,
		explicitly_killed: task.explicitlyKilled,
		owner_session_id: undefined,
	};
}

function systemTime(milliseconds: number) {
	return {
		secs_since_epoch: Math.floor(milliseconds / 1000),
		nanos_since_epoch: Math.floor(milliseconds % 1000) * 1_000_000,
	};
}

function taskResult(task: BackgroundTask) {
	const ended = task.endedAt === undefined ? undefined : new Date(task.endedAt).toISOString();
	return {
		task_id: task.taskId,
		command: task.command,
		status: taskState(task),
		exit_code: task.exitCode,
		started: new Date(task.startedAt).toISOString(),
		ended,
		duration_secs: ((task.endedAt ?? Date.now()) - task.startedAt) / 1000,
		output: task.output.toString("utf8"),
		output_file: task.outputFile,
		truncated: task.truncated,
		raw_output_bytes: task.outputBytes,
	};
}

function appendOutput(task: BackgroundTask, chunk: Buffer) {
	task.outputBytes += chunk.length;
	const joined = Buffer.concat([task.output, chunk]);
	if (joined.length > MAX_OUTPUT_BYTES) {
		task.output = joined.subarray(joined.length - MAX_OUTPUT_BYTES);
		task.truncated = true;
		return;
	}
	task.output = joined;
}

function killProcessTree(task: BackgroundTask) {
	const pid = task.child.pid;
	if (!pid) return;
	if (process.platform !== "win32") {
		try {
			process.kill(-pid, "SIGKILL");
			return;
		} catch {
			// The process may not own a group. Fall back to its direct PID.
		}
	}
	try {
		task.child.kill("SIGKILL");
	} catch {
		// The close handler will establish final task state when it is still alive.
	}
}

function waitForCompletion(task: BackgroundTask, timeoutMs: number | undefined, signal: AbortSignal | undefined) {
	if (task.completed) return Promise.resolve();
	return new Promise<void>((resolve, reject) => {
		let timer: ReturnType<typeof setTimeout> | undefined;
		const done = () => {
			if (timer) clearTimeout(timer);
			signal?.removeEventListener("abort", aborted);
			task.waiters.delete(done);
			resolve();
		};
		const aborted = () => {
			if (timer) clearTimeout(timer);
			task.waiters.delete(done);
			reject(new Error("aborted"));
		};
		if (signal?.aborted) {
			aborted();
			return;
		}
		task.waiters.add(done);
		signal?.addEventListener("abort", aborted, { once: true });
		if (timeoutMs && timeoutMs > 0) timer = setTimeout(done, timeoutMs);
	});
}

function emitCompleted(pi: ExtensionAPI, task: BackgroundTask) {
	pi.sendMessage(
		{
			customType: BRIDGE_TYPE,
			content: "",
			display: false,
			details: {
				version: 1,
				event: "completed",
				taskId: task.taskId,
				toolCallId: task.toolCallId,
				taskSnapshot: taskSnapshot(task),
			},
		},
		{ triggerTurn: false },
	);
}

function finishTask(pi: ExtensionAPI, task: BackgroundTask, code: number | null, signal: NodeJS.Signals | null) {
	if (task.completed) return;
	task.completed = true;
	task.endedAt = Date.now();
	task.exitCode = code ?? undefined;
	task.signal ??= signal ?? undefined;
	if (task.timeoutHandle) clearTimeout(task.timeoutHandle);
	task.log.end(() => {
		if (task.backgrounded) emitCompleted(pi, task);
		const settleForeground = task.foregroundSettler;
		task.foregroundSettler = undefined;
		settleForeground?.("completed");
		for (const waiter of task.waiters) waiter();
		task.waiters.clear();
		task.stateChanged?.();
	});
}

function launchShell(command: string, cwd: string, env: NodeJS.ProcessEnv) {
	const shell = process.platform === "win32" ? "bash" : "/bin/bash";
	return spawn(shell, ["-c", command], {
		cwd,
		env,
		detached: process.platform !== "win32",
		stdio: ["ignore", "pipe", "pipe"],
		windowsHide: true,
	});
}

function validateTimeout(timeout: number | undefined) {
	if (timeout === undefined) return;
	if (!Number.isFinite(timeout) || timeout <= 0 || timeout > MAX_TIMEOUT_SECONDS) {
		throw new Error(`Invalid timeout: must be a finite number of seconds up to ${MAX_TIMEOUT_SECONDS}`);
	}
}

async function startTask(
	pi: ExtensionAPI,
	params: {
		toolCallId: string;
		command: string;
		description?: string;
		cwd: string;
		timeout?: number;
		backgrounded: boolean;
		env: NodeJS.ProcessEnv;
		onData?: (chunk: Buffer) => void;
		stateChanged?: () => void;
	},
): Promise<BackgroundTask> {
	validateTimeout(params.timeout);
	const directory = await mkdtemp(join(tmpdir(), "pi-grok-bash-"));
	const task: BackgroundTask = {
		taskId: `bash-${randomUUID()}`,
		toolCallId: params.toolCallId,
		command: params.command,
		description: params.description?.trim() || undefined,
		cwd: params.cwd,
		outputFile: join(directory, "output.log"),
		startedAt: Date.now(),
		child: launchShell(params.command, params.cwd, params.env),
		log: createWriteStream(join(directory, "output.log"), { flags: "a" }),
		output: Buffer.alloc(0),
		outputBytes: 0,
		truncated: false,
		completed: false,
		backgrounded: params.backgrounded,
		explicitlyKilled: false,
		timedOut: false,
		waiters: new Set(),
		stateChanged: params.stateChanged,
	};
	const recordOutput = (chunk: Buffer) => {
		appendOutput(task, chunk);
		task.log.write(chunk);
		params.onData?.(chunk);
	};
	task.child.stdout?.on("data", recordOutput);
	task.child.stderr?.on("data", recordOutput);
	task.log.on("error", (error) => {
		task.signal ??= `output_log_error:${error.message}`;
	});
	task.child.once("error", (error) => {
		task.signal = error.message;
		finishTask(pi, task, null, null);
	});
	task.child.once("close", (code, childSignal) => finishTask(pi, task, code, childSignal));
	if (params.timeout) {
		task.timeoutHandle = setTimeout(() => {
			task.timedOut = true;
			task.signal = "timeout";
			killProcessTree(task);
		}, params.timeout * 1000);
	}
	return task;
}

function createBashControl(tasks: Map<string, BackgroundTask>): BashControl {
	const metaPath = process.env.PI_GROK_BASH_CONTROL_META;
	if (!metaPath) return { sync: () => {}, close: () => {} };

	const controlPath = join(tmpdir(), `pi-grok-bash-control-${randomUUID()}.jsonl`);
	closeSync(openSync(controlPath, "a"));
	let offset = 0;
	const sync = () => {
		const activeToolCallIds = [...tasks.values()]
			.filter((task) => !task.completed && !task.backgrounded && task.promote)
			.map((task) => task.toolCallId);
		try {
			writeFileSync(metaPath, JSON.stringify({ controlPath, activeToolCallIds }), "utf8");
		} catch {
			// A failed control publication only disables Pager promotion; Bash itself remains valid.
		}
	};
	const drain = () => {
		try {
			if (!existsSync(controlPath)) return;
			const content = readFileSync(controlPath, "utf8");
			if (content.length <= offset) return;
			const chunk = content.slice(offset);
			offset = content.length;
			for (const line of chunk.split("\n")) {
				if (!line.trim()) continue;
				try {
					const event = JSON.parse(line) as { op?: string; toolCallId?: string };
					if (event.op !== "background" || typeof event.toolCallId !== "string") continue;
					tasks.get(event.toolCallId)?.promote?.();
				} catch {
					// Ignore malformed events rather than affecting an active Bash process.
				}
			}
		} catch {
			// The adapter may race session shutdown; subsequent writes can retry.
		}
	};
	let watcher: FSWatcher | undefined;
	let poller: ReturnType<typeof setInterval> | undefined;
	try {
		watcher = watch(controlPath, drain);
	} catch {
		poller = setInterval(drain, 50);
	}
	sync();
	return {
		sync,
		close: () => {
			try {
				watcher?.close();
			} catch {
				// Ignore a watcher that already closed during shutdown.
			}
			if (poller) clearInterval(poller);
			try {
				if (existsSync(controlPath)) unlinkSync(controlPath);
			} catch {
				// The OS will clean the process temp directory on exit if needed.
			}
		},
	};
}

function ensureTaskIds(taskIds: string[]) {
	const ids = [...new Set(taskIds.map((id) => id.trim()).filter(Boolean))];
	if (ids.length === 0) throw new Error("task_ids must contain at least one task ID");
	if (ids.length > MAX_TASK_IDS) throw new Error(`task_ids may contain at most ${MAX_TASK_IDS} IDs`);
	return ids;
}

function jsonContent(value: unknown) {
	return [{ type: "text" as const, text: JSON.stringify(value, null, 2) }];
}

export default function (pi: ExtensionAPI) {
	const tasks = new Map<string, BackgroundTask>();
	const control = createBashControl(tasks);
	const nativeBash = createBashToolDefinition(process.cwd());
	const BashParameters = Type.Object({
		command: Type.String({ description: "Bash command to execute" }),
		timeout: Type.Optional(Type.Number({ description: "Timeout in seconds (optional, no default timeout)" })),
		is_background: Type.Optional(Type.Boolean({ description: "Run as a background task and return its task ID" })),
		description: Type.Optional(Type.String({ description: "Human-readable purpose for a background task" })),
	});

	pi.registerTool({
		...nativeBash,
		parameters: BashParameters,
		description: `${nativeBash.description} Set is_background to true for a managed background task.`,
		async execute(toolCallId, params: BashParams, signal, onUpdate, ctx: ExtensionContext) {
			if (signal?.aborted) throw new Error("aborted");
			if (params.is_background) {
				const task = await startTask(pi, {
					toolCallId,
					command: params.command,
					description: params.description,
					cwd: ctx.cwd,
					timeout: params.timeout,
					backgrounded: true,
					env: process.env,
					stateChanged: control.sync,
				});
				tasks.set(task.toolCallId, task);
				control.sync();
				return {
					content: jsonContent({ task_id: task.taskId, status: "running", output_file: task.outputFile }),
					details: {
						taskId: task.taskId,
						background: true,
						command: task.command,
						cwd: task.cwd,
						outputFile: task.outputFile,
						description: task.description,
					},
				};
			}

			let task: BackgroundTask | undefined;
			const managedBash = createBashToolDefinition(ctx.cwd, {
				operations: {
					exec: async (command, cwd, options) => {
						task = await startTask(pi, {
							toolCallId,
							command,
							cwd,
							timeout: options.timeout,
							backgrounded: false,
							env: options.env ?? process.env,
							onData: options.onData,
							stateChanged: control.sync,
						});
						tasks.set(toolCallId, task);
						const activeTask = task;
						return new Promise<{ exitCode: number | null }>((resolve, reject) => {
							const settle = (outcome: "completed" | "backgrounded") => {
								activeTask.foregroundSettler = undefined;
								activeTask.promote = undefined;
								options.signal?.removeEventListener("abort", aborted);
								if (outcome === "backgrounded") {
									resolve({ exitCode: 0 });
									return;
								}
								if (options.signal?.aborted) {
									reject(new Error("aborted"));
									return;
								}
								if (activeTask.timedOut) {
									reject(new Error(`timeout:${options.timeout}`));
									return;
								}
								resolve({ exitCode: activeTask.exitCode ?? null });
							};
							const aborted = () => killProcessTree(activeTask);
							activeTask.foregroundSettler = settle;
							activeTask.promote = () => {
								if (activeTask.completed || activeTask.backgrounded) return;
								activeTask.backgrounded = true;
								control.sync();
								settle("backgrounded");
							};
							if (options.signal?.aborted) {
								aborted();
							} else {
								options.signal?.addEventListener("abort", aborted, { once: true });
							}
							control.sync();
							if (activeTask.completed) settle("completed");
						});
					},
				},
			});
			try {
				const result = await managedBash.execute(toolCallId, params, signal, onUpdate, ctx);
				if (!task?.backgrounded) return result;
				return {
					...result,
					details: {
						...result.details,
						taskId: task.taskId,
						background: true,
						command: task.command,
						cwd: task.cwd,
						outputFile: task.outputFile,
					},
				};
			} finally {
				if (task && !task.backgrounded) {
					tasks.delete(task.toolCallId);
					control.sync();
				}
			}
		},
	});

	pi.registerTool({
		name: "get_task_output",
		label: "get_task_output",
		description: "Get output for one or more background bash tasks. Set timeout_ms to wait for completion; omit it to poll.",
		parameters: Type.Object({
			task_ids: Type.Array(Type.String({ minLength: 1 })),
			timeout_ms: Type.Optional(Type.Number({ minimum: 0 })),
		}),
		async execute(_toolCallId, params, signal) {
			const ids = ensureTaskIds(params.task_ids);
			const selected = ids.map((id) => [...tasks.values()].find((task) => task.taskId === id));
			if (selected.some((task) => !task)) return { content: jsonContent({ task_not_found: ids.filter((id) => !selected.find((task) => task?.taskId === id)) }) };
			if (params.timeout_ms && params.timeout_ms > 0) {
				await Promise.all(selected.map((task) => waitForCompletion(task!, params.timeout_ms, signal)));
			}
			const results = selected.map((task) => taskResult(task!));
			return { content: jsonContent(results.length === 1 ? results[0] : { mode: "wait_all", results }) };
		},
	});

	pi.registerTool({
		name: "wait_tasks",
		label: "wait_tasks",
		description: "Wait for background bash tasks to finish.",
		parameters: Type.Object({
			task_ids: Type.Array(Type.String({ minLength: 1 })),
			mode: StringEnum(["wait_any", "wait_all"] as const),
			timeout_ms: Type.Optional(Type.Number({ minimum: 0 })),
		}),
		async execute(_toolCallId, params, signal) {
			const ids = ensureTaskIds(params.task_ids);
			const selected = ids.map((id) => [...tasks.values()].find((task) => task.taskId === id));
			if (selected.some((task) => !task)) return { content: jsonContent({ task_not_found: ids.filter((id) => !selected.find((task) => task?.taskId === id)) }) };
			const waits = selected.map((task) => waitForCompletion(task!, params.timeout_ms, signal));
			if (params.mode === "wait_any") await Promise.race(waits);
			else await Promise.all(waits);
			const results = selected.map((task) => taskResult(task!));
			return { content: jsonContent({ mode: params.mode, results }) };
		},
	});

	pi.registerTool({
		name: "kill_task",
		label: "kill_task",
		description: "Terminate a running background bash task by task ID.",
		parameters: Type.Object({ task_id: Type.String({ minLength: 1 }) }),
		async execute(_toolCallId, params) {
			const task = [...tasks.values()].find((candidate) => candidate.taskId === params.task_id.trim());
			if (!task) return { content: jsonContent({ task_not_found: params.task_id }) };
			if (task.completed) return { content: jsonContent({ task_id: task.taskId, outcome: "already_exited" }) };
			task.explicitlyKilled = true;
			task.signal = "killed";
			killProcessTree(task);
			return { content: jsonContent({ task_id: task.taskId, outcome: "killed" }) };
		},
	});

	pi.on("session_shutdown", () => {
		control.close();
		for (const task of tasks.values()) {
			if (task.completed) continue;
			task.explicitlyKilled = true;
			task.signal = "session_shutdown";
			killProcessTree(task);
		}
	});
}
