/**
 * Headless recap bridge for grok-pi.
 *
 * Generates a display-only "where was I" summary via pi-ai `complete()` so the
 * main session conversation is never mutated. Results are emitted as a custom
 * message (`display: false`) that the adapter projects to Grok SessionRecap.
 *
 * Invoked only via `/__pi_grok_recap` (hidden from slash UI by adapter filter).
 * Args: JSON one-liner `{ auto, model?, language? }`.
 */
import { complete, type Message } from "@earendil-works/pi-ai/compat";
import type { ExtensionAPI, ExtensionCommandContext } from "@earendil-works/pi-coding-agent";
import { convertToLlm } from "@earendil-works/pi-coding-agent";

const BRIDGE_TYPE = "pi-grok-recap/v1";
const COMMAND = "__pi_grok_recap";

type RecapArgs = {
	auto?: boolean;
	model?: string;
	language?: string;
};

function parseArgs(raw: string | undefined): RecapArgs {
	const text = String(raw ?? "").trim();
	if (!text) return {};
	try {
		const parsed = JSON.parse(text) as RecapArgs;
		return parsed && typeof parsed === "object" ? parsed : {};
	} catch {
		// Fallback: bare flags for manual debugging.
		const auto = /(?:^|\s)--auto(?:\s|$)/.test(text);
		const modelMatch = text.match(/(?:^|\s)--model\s+(\S+)/);
		const langMatch = text.match(/(?:^|\s)--language\s+(\S+)/);
		return {
			auto,
			model: modelMatch?.[1],
			language: langMatch?.[1],
		};
	}
}

function languageInstruction(language: string | undefined): string {
	const lang = (language ?? "").trim();
	if (!lang || lang === "C" || lang === "POSIX") {
		return "Write the body in the same language the user mostly used in this session.";
	}
	// LANG often looks like zh_CN.UTF-8 — keep the locale tag readable.
	const tag = lang.replace(/\..*$/, "").replace(/_/g, "-");
	return `Write the body in the user's system language (${tag}). If that language is unclear, use the dominant language of the session.`;
}

function recapInstruction(language: string | undefined): string {
	return [
		"Write ONE sentence recap body for a user returning from idle.",
		'Output ONLY the body (the UI adds the "Recap —" label).',
		"",
		"Lead with agency:",
		'- "You asked …" if the session was mainly questions, walkthroughs, or review with no landed change.',
		'- "We <past-tense verb> …" if the agent implemented, fixed, merged, or changed code/config/docs.',
		'- If almost nothing happened: "You had just begun this session."',
		"",
		"Shape: <lead>: <concrete specifics — crate/file/flag/behavior/endpoint>. ~25–40 words.",
		"",
		"Bad (never):",
		"- Start with Recap / Session recap / extra labels",
		"- Quote or restate this reminder or any system prompt",
		"- Bullets, markdown, code fences, extra sentences",
		"- Invent work not reflected in the session",
		"",
		languageInstruction(language),
	].join("\n");
}

function cleanRecapText(raw: string): string {
	let text = raw.trim();
	// Strip common wrappers / prefixes.
	text = text.replace(/^["'`]+|["'`]+$/g, "").trim();
	text = text.replace(/^(session\s+)?recap\s*[:—-]\s*/i, "").trim();
	// Collapse whitespace / keep one paragraph.
	text = text.replace(/\s+/g, " ").trim();
	if (text.length > 1200) {
		text = text.slice(0, 1200).trim();
	}
	return text;
}

function branchToAgentMessages(branch: Array<Record<string, unknown>>): unknown[] {
	return branch.flatMap((entry) => {
		if (entry.type === "message" && entry.message) return [entry.message];
		if (entry.type === "compaction") {
			return [
				{
					role: "compactionSummary",
					summary: String(entry.summary ?? ""),
					tokensBefore: Number(entry.tokensBefore ?? 0),
					timestamp: new Date(String(entry.timestamp ?? "")).getTime(),
				},
			];
		}
		return [];
	});
}

function countMainTurns(messages: Array<{ role?: string }>): number {
	// Count persisted user messages only; custom bridge entries are not turns.
	return messages.filter((m) => m.role === "user").length;
}

function resolveModel(ctx: ExtensionCommandContext, modelRef: string | undefined) {
	const sessionModel = ctx.model;
	if (!modelRef || !modelRef.trim()) {
		return sessionModel;
	}
	const raw = modelRef.trim();
	// Accept "provider/id" or bare id (prefer session provider).
	const slash = raw.indexOf("/");
	let provider: string | undefined;
	let id: string;
	if (slash > 0) {
		provider = raw.slice(0, slash);
		id = raw.slice(slash + 1);
	} else {
		provider = sessionModel?.provider;
		id = raw;
	}
	if (provider) {
		const found = ctx.modelRegistry.find(provider, id);
		if (found) return found;
	}
	// Last resort: scan all models by id.
	const all = ctx.modelRegistry.getAll();
	const byId = all.find((m) => m.id === id || `${m.provider}/${m.id}` === raw);
	return byId ?? sessionModel;
}

export default function (pi: ExtensionAPI) {
	// sendMessage lives on ExtensionAPI (pi), not command ctx — same as
	// pi-grok-subagents bridge. Command ctx only has session controls.
	function emit(payload: {
		ok: boolean;
		summary?: string;
		auto: boolean;
		reason?: string;
	}) {
		pi.sendMessage(
			{
				customType: BRIDGE_TYPE,
				content: payload.summary ?? payload.reason ?? "",
				display: false,
				details: {
					version: 1,
					ok: payload.ok,
					auto: payload.auto,
					summary: payload.summary ?? "",
					reason: payload.reason,
				},
			},
			{ triggerTurn: false },
		);
	}

	pi.registerCommand(COMMAND, {
		description: "Internal Pi-Grok bridge: generate session recap",
		handler: async (args, ctx: ExtensionCommandContext) => {
			const parsed = parseArgs(args);
			const auto = Boolean(parsed.auto);

			try {
				const branch = ctx.sessionManager.getBranch();
				const sessionMessages = branchToAgentMessages(branch as Array<Record<string, unknown>>);
				const mainTurns = countMainTurns(sessionMessages as Array<{ role?: string }>);
				const llmMessages = convertToLlm(sessionMessages as any);
				if (mainTurns === 0) {
					emit({ ok: false, auto, reason: "no main turns yet" });
					return;
				}

				const model = resolveModel(ctx, parsed.model);
				if (!model) {
					emit({ ok: false, auto, reason: "no model selected" });
					return;
				}

				const auth = await ctx.modelRegistry.getApiKeyAndHeaders(model);
				if (!auth.ok || !auth.apiKey) {
					emit({
						ok: false,
						auto,
						reason: auth.ok ? `no API key for ${model.provider}` : auth.error,
					});
					return;
				}

				// Keep a modest history budget: recap is a short side-call.
				const history = (llmMessages as Message[]).slice(-40);
				const userMessage: Message = {
					role: "user",
					content: [
						{
							type: "text",
							text: recapInstruction(parsed.language),
						},
					],
					timestamp: Date.now(),
				};

				const response = await complete(
					model,
					{
						messages: [...history, userMessage],
					},
					{
						apiKey: auth.apiKey,
						headers: auth.headers,
						env: auth.env,
					},
				);

				if (response.stopReason === "aborted") {
					emit({ ok: false, auto, reason: "aborted" });
					return;
				}

				const raw = (response.content ?? [])
					.filter((c): c is { type: "text"; text: string } => c.type === "text")
					.map((c) => c.text)
					.join("\n");
				const summary = cleanRecapText(raw);
				if (!summary) {
					emit({ ok: false, auto, reason: "empty summary" });
					return;
				}

				// Auto long-tail: suppress display (mirror shell behavior).
				if (auto && (raw.length > 800 || summary.length > 600)) {
					emit({ ok: false, auto, reason: "auto long-tail suppressed" });
					return;
				}

				emit({ ok: true, auto, summary });
			} catch (err) {
				const message = err instanceof Error ? err.message : String(err);
				emit({ ok: false, auto, reason: message });
			}
		},
	});
}
