/**
 * pi-grok-00-profiler — Startup performance profiler extension.
 *
 * Loads FIRST (00- prefix) so monkey-patches are in place before other
 * extensions activate. Measures:
 *   1. What each extension registers (tools/commands/events/flags/shortcuts/providers)
 *   2. Event dispatch timing across the full lifecycle
 *   3. Resource usage (memory, CPU)
 *   4. Which phase is slowest
 *
 * Enable: PI_PROFILE=1 env var, or loaded via grok-pi bridge (auto-enabled)
 * Report: /profile command opens scrollable TUI overlay; 'e' exports to ~/Downloads
 */

import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { ExtensionRunner } from "@earendil-works/pi-coding-agent";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

interface EventRecord {
	type: string;
	offsetMs: number;
	totalMs: number;
	handlerCount: number;
	perExtension: Array<{ name: string; ms: number }>;
}

interface ExtensionSnapshot {
	name: string;
	path: string;
	tools: string[];
	commands: string[];
	events: string[];
	flags: string[];
	shortcuts: string[];
	providers: string[];
}

interface ResourceSnapshot {
	label: string;
	offsetMs: number;
	rssMB: number;
	heapUsedMB: number;
	heapTotalMB: number;
	externalMB: number;
}

interface ProfilerState {
	enabled: boolean;
	t0: number;
	memAtLoad: NodeJS.MemoryUsage;
	events: EventRecord[];
	extensions: ExtensionSnapshot[];
	resources: ResourceSnapshot[];
	patched: boolean;
	sessionStartMs: number;
	cwd: string;
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

const state: ProfilerState = {
	enabled: true, // Always enabled when loaded (grok-pi controls loading)
	t0: Date.now(),
	memAtLoad: process.memoryUsage(),
	events: [],
	extensions: [],
	resources: [],
	patched: false,
	sessionStartMs: 0,
	cwd: process.cwd(),
};

state.resources.push({
	label: "profiler-load",
	offsetMs: 0,
	rssMB: state.memAtLoad.rss / 1048576,
	heapUsedMB: state.memAtLoad.heapUsed / 1048576,
	heapTotalMB: state.memAtLoad.heapTotal / 1048576,
	externalMB: state.memAtLoad.external / 1048576,
});

// ---------------------------------------------------------------------------
// Monkey-patch ExtensionRunner.prototype
// ---------------------------------------------------------------------------

const proto = ExtensionRunner.prototype as Record<string, unknown>;

function extName(p: string): string {
	const parts = p.replace(/\\/g, "/").split("/");
	const last = parts[parts.length - 1] ?? p;
	return last.replace(/\.(ts|js)$/, "");
}

function snapshotExtensions(runner: unknown): void {
	const r = runner as { extensions?: Array<Record<string, unknown>> };
	if (!r.extensions) return;
	state.extensions = r.extensions.map((ext) => ({
		name: extName(String(ext.path ?? "")),
		path: String(ext.path ?? ""),
		tools: ext.tools instanceof Map ? [...(ext.tools as Map<string, unknown>).keys()] : [],
		commands: ext.commands instanceof Map ? [...(ext.commands as Map<string, unknown>).keys()] : [],
		events: ext.handlers instanceof Map ? [...(ext.handlers as Map<string, unknown[]>).keys()] : [],
		flags: ext.flags instanceof Map ? [...(ext.flags as Map<string, unknown>).keys()] : [],
		shortcuts: ext.shortcuts instanceof Map ? [...(ext.shortcuts as Map<string, unknown>).keys()] : [],
		providers: [],
	}));
}

function patchRunner(): void {
	if (state.patched) return;
	state.patched = true;

	const origBindCore = proto.bindCore as ((...args: unknown[]) => unknown) | undefined;
	if (origBindCore) {
		proto.bindCore = function (this: unknown, ...args: unknown[]) {
			const result = origBindCore.apply(this, args);
			snapshotExtensions(this);
			return result;
		};
	}

	const EMIT_METHODS = [
		"emit",
		"emitMessageEnd",
		"emitToolResult",
		"emitToolCall",
		"emitUserBash",
		"emitContext",
		"emitBeforeProviderRequest",
		"emitBeforeProviderHeaders",
		"emitBeforeAgentStart",
		"emitResourcesDiscover",
		"emitInput",
	];

	for (const method of EMIT_METHODS) {
		const orig = proto[method] as ((...args: unknown[]) => Promise<unknown>) | undefined;
		if (!orig) continue;

		proto[method] = async function (this: unknown, ...args: unknown[]) {
			if (!state.enabled) return orig.apply(this, args);

			const eventArg = args[0] as { type?: string } | undefined;
			const eventType = eventArg?.type ?? method;
			const start = performance.now();

			const result = await orig.apply(this, args);

			const ms = performance.now() - start;
			let handlerCount = 0;
			const perExtension: Array<{ name: string; ms: number }> = [];
			const r = this as { extensions?: Array<Record<string, unknown>> };
			if (r.extensions) {
				for (const ext of r.extensions) {
					const handlers =
						ext.handlers instanceof Map
							? (ext.handlers as Map<string, unknown[]>).get(eventType)
							: undefined;
					if (handlers && handlers.length > 0) {
						handlerCount += handlers.length;
						perExtension.push({ name: extName(String(ext.path ?? "")), ms: 0 });
					}
				}
			}

			state.events.push({
				type: eventType,
				offsetMs: Date.now() - state.t0,
				totalMs: Math.round(ms * 100) / 100,
				handlerCount,
				perExtension,
			});

			return result;
		};
	}
}

patchRunner();

// ---------------------------------------------------------------------------
// Report rendering
// ---------------------------------------------------------------------------

function renderReport(): string {
	const lines: string[] = [];
	const totalMs = state.sessionStartMs > 0 ? state.sessionStartMs : Date.now() - state.t0;

	lines.push(`Startup Profile — Total: ${totalMs}ms | Extensions: ${state.extensions.length} | Events: ${state.events.length}`);
	lines.push("");

	// Phase tree
	lines.push("═══ Phase Tree ═══");
	const phases = new Map<string, { ms: number; count: number }>();
	for (const ev of state.events) {
		const phase = ev.type.replace(/_.*/, "") || ev.type;
		const existing = phases.get(phase) ?? { ms: 0, count: 0 };
		existing.ms += ev.totalMs;
		existing.count += 1;
		phases.set(phase, existing);
	}
	const sortedPhases = [...phases.entries()].sort((a, b) => b[1].ms - a[1].ms);
	for (let i = 0; i < sortedPhases.length; i++) {
		const [phase, data] = sortedPhases[i];
		const isLast = i === sortedPhases.length - 1;
		const connector = isLast ? "└─" : "├─";
		const pct = totalMs > 0 ? Math.round((data.ms / totalMs) * 100) : 0;
		const bar = "█".repeat(Math.min(20, Math.round(pct / 5)));
		lines.push(`${connector} ${phase.padEnd(26)} ${String(Math.round(data.ms)).padStart(6)}ms  ${bar} ${pct}%`);
	}
	lines.push("");

	// Extension registrations
	if (state.extensions.length > 0) {
		lines.push("═══ Extension Registrations ═══");
		for (const ext of state.extensions) {
			const parts: string[] = [];
			if (ext.tools.length) parts.push(`${ext.tools.length} tools`);
			if (ext.commands.length) parts.push(`${ext.commands.length} cmds`);
			if (ext.events.length) parts.push(`${ext.events.length} events`);
			if (ext.flags.length) parts.push(`${ext.flags.length} flags`);
			if (ext.shortcuts.length) parts.push(`${ext.shortcuts.length} keys`);
			if (ext.providers.length) parts.push(`${ext.providers.length} providers`);
			lines.push(`  ${ext.name.padEnd(30)} → ${parts.length > 0 ? parts.join(", ") : "(none)"}`);
			if (ext.tools.length > 0 && ext.tools.length <= 10) {
				lines.push(`    tools: ${ext.tools.join(", ")}`);
			}
			if (ext.commands.length > 0 && ext.commands.length <= 10) {
				lines.push(`    cmds:  ${ext.commands.join(", ")}`);
			}
		}
		lines.push("");
	}

	// Event timeline
	if (state.events.length > 0) {
		lines.push("═══ Event Timeline ═══");
		for (const ev of state.events) {
			lines.push(
				`  +${String(ev.offsetMs).padStart(6)}ms  ${ev.type.padEnd(30)} ${String(ev.handlerCount).padStart(2)}h  ${String(ev.totalMs).padStart(7)}ms`,
			);
		}
		lines.push("");
	}

	// Resource usage
	lines.push("═══ Resource Usage ═══");
	const memNow = process.memoryUsage();
	lines.push(`  RSS:        ${(memNow.rss / 1048576).toFixed(1)} MB`);
	lines.push(`  Heap used:  ${(memNow.heapUsed / 1048576).toFixed(1)} MB / ${(memNow.heapTotal / 1048576).toFixed(1)} MB`);
	lines.push(`  External:   ${(memNow.external / 1048576).toFixed(1)} MB`);
	if (typeof process.resourceUsage === "function") {
		const ru = process.resourceUsage();
		lines.push(`  CPU user:   ${(ru.userCPUTime / 1000).toFixed(1)} ms`);
		lines.push(`  CPU sys:    ${(ru.systemCPUTime / 1000).toFixed(1)} ms`);
		lines.push(`  Max RSS:    ${(ru.maxRSS / 1024).toFixed(1)} MB`);
	}
	const heapDelta = memNow.heapUsed - state.memAtLoad.heapUsed;
	lines.push(`  Heap Δ:     ${(heapDelta / 1048576).toFixed(1)} MB since load`);
	lines.push("");

	// Insights
	const insights = computeInsights(totalMs);
	if (insights.length > 0) {
		lines.push("═══ Insights ═══");
		for (const ins of insights) {
			lines.push(`  ${ins}`);
		}
		lines.push("");
	}

	lines.push("─── [q/Esc close] [e export] [j/↓ k/↑ scroll] ───");
	return lines.join("\n");
}

function computeInsights(totalMs: number): string[] {
	const insights: string[] = [];

	if (state.events.length > 0) {
		const slowest = state.events.reduce((a, b) => (a.totalMs > b.totalMs ? a : b));
		if (slowest.totalMs > 10) {
			insights.push(`🐢 Slowest event: ${slowest.type} (${slowest.totalMs}ms, ${slowest.handlerCount} handlers)`);
		}
	}

	if (state.extensions.length > 0) {
		const heaviest = state.extensions.reduce((a, b) => {
			const cA = a.tools.length + a.commands.length + a.events.length;
			const cB = b.tools.length + b.commands.length + b.events.length;
			return cA > cB ? a : b;
		});
		const totalRegs = heaviest.tools.length + heaviest.commands.length + heaviest.events.length;
		if (totalRegs > 5) {
			insights.push(`📦 Heaviest: ${heaviest.name} → ${heaviest.tools.length} tools, ${heaviest.commands.length} cmds, ${heaviest.events.length} events`);
		}
	}

	const eventSubCount = new Map<string, number>();
	for (const ext of state.extensions) {
		for (const ev of ext.events) {
			eventSubCount.set(ev, (eventSubCount.get(ev) ?? 0) + 1);
		}
	}
	for (const [ev, count] of eventSubCount) {
		if (count >= 4) {
			insights.push(`⚠️  Hot event: ${ev} subscribed by ${count} extensions`);
		}
	}

	const eventTotalMs = state.events.reduce((s, e) => s + e.totalMs, 0);
	if (totalMs > 0 && eventTotalMs / totalMs > 0.3) {
		insights.push(`⏱️  Event dispatch: ${Math.round((eventTotalMs / totalMs) * 100)}% of startup`);
	}

	const memNow = process.memoryUsage();
	const heapGrowth = (memNow.heapUsed - state.memAtLoad.heapUsed) / 1048576;
	if (heapGrowth > 20) {
		insights.push(`🧠 Heap grew ${heapGrowth.toFixed(1)} MB during startup`);
	}

	return insights;
}

// ---------------------------------------------------------------------------
// Export
// ---------------------------------------------------------------------------

function exportReport(): string {
	const now = new Date();
	const ts = now.toISOString().replace(/[:.]/g, "-").slice(0, 19);
	const cwdSlug = state.cwd.replace(/[/\\]/g, "_").replace(/^_/, "").slice(-30);
	const filename = `pi-profile-${ts}-${cwdSlug}.txt`;
	const dir = path.join(os.homedir(), "Downloads");
	const filePath = path.join(dir, filename);

	const header = [
		`Pi Startup Profile`,
		`Generated: ${now.toISOString()}`,
		`CWD: ${state.cwd}`,
		`Extensions: ${state.extensions.length}`,
		`Total startup: ${state.sessionStartMs > 0 ? state.sessionStartMs : Date.now() - state.t0}ms`,
		"=".repeat(70),
		"",
	].join("\n");

	const content = header + renderReport();
	fs.mkdirSync(dir, { recursive: true });
	fs.writeFileSync(filePath, content, "utf-8");
	return filePath;
}

// ---------------------------------------------------------------------------
// Scrollable overlay component
// ---------------------------------------------------------------------------

interface TuiLike {
	showOverlay(component: unknown, options?: unknown): { hide(): void };
	terminal?: { columns: number; rows: number };
	requestRender?(): void;
}

function createScrollableReport(tui: TuiLike): { component: unknown; close(): void } {
	const reportLines = renderReport().split("\n");
	let scrollOffset = 0;
	let overlayHandle: { hide(): void } | null = null;

	const viewHeight = () => {
		const rows = tui.terminal?.rows ?? 40;
		return Math.max(10, rows - 6);
	};

	const component = {
		render(width: number): string[] {
			const h = viewHeight();
			const maxScroll = Math.max(0, reportLines.length - h);
			scrollOffset = Math.min(scrollOffset, maxScroll);
			const visible = reportLines.slice(scrollOffset, scrollOffset + h);
			// Pad to fill height
			while (visible.length < h) visible.push("");
			return visible;
		},
		handleInput(data: string): void {
			const h = viewHeight();
			const maxScroll = Math.max(0, reportLines.length - h);
			if (data === "q" || data === "\x1b" || data === "\x03") {
				// q, Escape, Ctrl-C → close
				overlayHandle?.hide();
			} else if (data === "j" || data === "\x1b[B" || data === "\n") {
				scrollOffset = Math.min(scrollOffset + 1, maxScroll);
				tui.requestRender?.();
			} else if (data === "k" || data === "\x1b[A") {
				scrollOffset = Math.max(scrollOffset - 1, 0);
				tui.requestRender?.();
			} else if (data === "d" || data === "\x04") {
				// d or Ctrl-D → page down
				scrollOffset = Math.min(scrollOffset + h, maxScroll);
				tui.requestRender?.();
			} else if (data === "u" || data === "\x15") {
				// u or Ctrl-U → page up
				scrollOffset = Math.max(scrollOffset - h, 0);
				tui.requestRender?.();
			} else if (data === "g") {
				scrollOffset = 0;
				tui.requestRender?.();
			} else if (data === "G") {
				scrollOffset = maxScroll;
				tui.requestRender?.();
			} else if (data === "e") {
				// Export
				const filePath = exportReport();
				// Temporarily show export path at bottom
				reportLines[reportLines.length - 1] = `─── Exported: ${filePath} ───`;
				tui.requestRender?.();
			}
		},
		invalidate(): void {},
	};

	overlayHandle = tui.showOverlay(component, {
		width: "92%",
		maxHeight: "88%",
		anchor: "center",
	});

	return {
		component,
		close() {
			overlayHandle?.hide();
		},
	};
}

// ---------------------------------------------------------------------------
// Extension factory
// ---------------------------------------------------------------------------

export default function (pi: {
	on(event: string, handler: (...args: unknown[]) => unknown): void;
	registerCommand(name: string, options: Record<string, unknown>): void;
	registerFlag(
		name: string,
		options: { description?: string; type: "boolean" | "string"; default?: boolean | string },
	): void;
	getFlag(name: string): boolean | string | undefined;
}) {
	pi.registerFlag("profile-startup", {
		description: "Enable startup performance profiling with full terminal report",
		type: "boolean",
		default: false,
	});

	// Subscribe to all events for timeline coverage
	const ALL_EVENTS = [
		"session_start",
		"session_info_changed",
		"session_before_switch",
		"session_before_fork",
		"session_before_compact",
		"session_compact",
		"session_shutdown",
		"session_before_tree",
		"session_tree",
		"context",
		"before_provider_request",
		"before_provider_headers",
		"after_provider_response",
		"before_agent_start",
		"agent_start",
		"agent_end",
		"agent_settled",
		"turn_start",
		"turn_end",
		"message_start",
		"message_update",
		"message_end",
		"tool_execution_start",
		"tool_execution_update",
		"tool_execution_end",
		"model_select",
		"thinking_level_select",
		"tool_call",
		"tool_result",
		"user_bash",
		"input",
		"resources_discover",
	];

	for (const evt of ALL_EVENTS) {
		pi.on(evt, () => {});
	}

	// /profile command — export report + show summary widget
	pi.registerCommand("profile", {
		description: "Export startup profile to ~/Downloads and show summary",
		handler: async (_args: string, ctx: { ui: Record<string, unknown>; mode: string }) => {
			const report = renderReport();

			// Always export full report
			const exportedPath = exportReport(report);

			// Show summary as widget (string[] form works in all modes including grok-pi RPC bridge)
			const setWidget = ctx.ui.setWidget as (
				key: string,
				content: string[] | undefined,
				options?: unknown,
			) => void;

			const summaryLines = buildSummaryLines(exportedPath);
			setWidget("profiler-report", summaryLines);

			// Notify with export path
			const notify = ctx.ui.notify as ((msg: string, type?: string) => void) | undefined;
			notify?.(`Profile exported: ${exportedPath}`, "info");

			// Auto-clear widget after 30s
			setTimeout(() => {
				setWidget("profiler-report", undefined);
			}, 30000);
		},
	});

	// Auto-output on session_start
	pi.on("session_start", (_event: unknown, ctx: unknown) => {
		state.sessionStartMs = Date.now() - state.t0;
		const c = ctx as { cwd?: string };
		if (c.cwd) state.cwd = c.cwd;

		const mem = process.memoryUsage();
		state.resources.push({
			label: "session-start",
			offsetMs: state.sessionStartMs,
			rssMB: mem.rss / 1048576,
			heapUsedMB: mem.heapUsed / 1048576,
			heapTotalMB: mem.heapTotal / 1048576,
			externalMB: mem.external / 1048576,
		});

		// Auto-export on startup
		const filePath = exportReport();

		const uiCtx = c as { ui?: { notify?: (msg: string, type?: string) => void } };
		if (uiCtx.ui?.notify) {
			uiCtx.ui.notify(`Profile exported: ${filePath} — use /profile to view`, "info");
		} else {
			console.error(`[profiler] Report exported: ${filePath}`);
		}
	});
}
