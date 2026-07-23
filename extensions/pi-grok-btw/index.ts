/**
 * Headless /btw bridge for grok-pi.
 *
 * Single-turn side question via pi-ai `complete()` — does not mutate the main
 * session conversation. Results are emitted as custom messages
 * (`pi-grok-btw/v1`, display:false) that the adapter projects to ACP x.ai/btw.
 *
 * Invoked only via `/__pi_grok_btw` (hidden from slash UI by adapter filter).
 * Args JSON: `{ requestId, question, models?: string[], thinkingLevel? }`.
 */
import { complete, type Message } from "@earendil-works/pi-ai/compat";
import type { ExtensionAPI, ExtensionCommandContext } from "@earendil-works/pi-coding-agent";

const BRIDGE_TYPE = "pi-grok-btw/v1";
const COMMAND = "__pi_grok_btw";
const MAX_CONTEXT_CHARS = 48_000;
const MAX_MESSAGE_CHARS = 4_000;

type BtwArgs = {
	requestId?: string;
	question?: string;
	models?: string[];
	model?: string;
	thinkingLevel?: "minimal" | "low" | "medium" | "high" | "xhigh" | "max";
};

function parseArgs(raw: string | undefined): BtwArgs {
	const text = String(raw ?? "").trim();
	if (!text) return {};
	try {
		const parsed = JSON.parse(text) as BtwArgs;
		return parsed && typeof parsed === "object" ? parsed : {};
	} catch {
		return { question: text };
	}
}

function truncateText(text: string, maxChars: number): string {
	const normalized = text.replace(/\s+/g, " ").trim();
	if (normalized.length <= maxChars) return normalized;
	return `${normalized.slice(0, maxChars).trimEnd()}…`;
}

function messageText(message: Record<string, unknown>): string {
	const content = message.content;
	if (typeof content === "string") return truncateText(content, MAX_MESSAGE_CHARS);
	if (!Array.isArray(content)) return "";
	const parts: string[] = [];
	for (const block of content) {
		if (!block || typeof block !== "object") continue;
		const item = block as Record<string, unknown>;
		if (item.type === "text" && typeof item.text === "string") parts.push(item.text);
		if (item.type === "toolCall" && typeof item.name === "string") {
			parts.push(`[tool: ${item.name}]`);
		}
		if (item.type === "toolResult") {
			const text =
				typeof item.text === "string"
					? item.text
					: typeof item.content === "string"
						? item.content
						: "";
			if (text) parts.push(`[tool result]: ${truncateText(text, 800)}`);
		}
	}
	return truncateText(parts.join("\n"), MAX_MESSAGE_CHARS);
}

/** Drop trailing incomplete assistant tool runs (mid-turn snapshot safety). */
function stripIncompleteTail(branch: Array<Record<string, unknown>>): Array<Record<string, unknown>> {
	const out = branch.slice();
	while (out.length > 0) {
		const last = out[out.length - 1];
		if (last.type === "message" && last.message && typeof last.message === "object") {
			const msg = last.message as Record<string, unknown>;
			const role = msg.role;
			if (role === "toolResult") {
				out.pop();
				continue;
			}
			if (role === "assistant") {
				const content = msg.content;
				const hasToolCall =
					Array.isArray(content) &&
					content.some(
						(b) =>
							b &&
							typeof b === "object" &&
							(b as Record<string, unknown>).type === "toolCall",
					);
				if (hasToolCall) {
					out.pop();
					continue;
				}
			}
		}
		break;
	}
	return out;
}

function buildSideContext(branch: Array<Record<string, unknown>>): string {
	const lines: string[] = [];
	const cleaned = stripIncompleteTail(branch);
	for (const entry of cleaned) {
		if (entry.type === "compaction") {
			const summary = truncateText(String(entry.summary ?? ""), 2_000);
			if (summary) lines.push(`[Earlier summary]: ${summary}`);
			continue;
		}
		if (entry.type !== "message" || !entry.message || typeof entry.message !== "object") {
			continue;
		}
		const message = entry.message as Record<string, unknown>;
		const role = message.role;
		if (role !== "user" && role !== "assistant" && role !== "toolResult" && role !== "system") {
			continue;
		}
		const text = messageText(message);
		if (!text) continue;
		const label =
			role === "user"
				? "User"
				: role === "assistant"
					? "Assistant"
					: role === "system"
						? "System"
						: "Tool result";
		lines.push(`[${label}]: ${text}`);
	}
	const context = lines.join("\n\n");
	if (context.length <= MAX_CONTEXT_CHARS) return context;
	const tail = context.slice(-MAX_CONTEXT_CHARS);
	const firstBoundary = tail.indexOf("\n\n");
	return firstBoundary >= 0 ? tail.slice(firstBoundary + 2) : tail;
}

function sideQuestionInstruction(question: string): string {
	return [
		"<system-reminder>",
		"This is a side question from the user.",
		"You must answer this question directly in a single response.",
		"",
		"IMPORTANT CONTEXT:",
		"- You are a separate, lightweight agent spawned to answer this one question",
		"- The main agent is NOT interrupted - it continues working independently in the background",
		"- You share the conversation context but are a completely separate instance",
		'- Do NOT reference being interrupted or what you were "previously doing" - that framing is incorrect',
		"",
		"CRITICAL CONSTRAINTS:",
		"- You have NO tools available - you cannot read files, run commands, search, or take any actions",
		"- This is a one-off response - there will be no follow-up turns",
		"- You can ONLY provide information based on what you already know from the conversation context",
		'- NEVER say things like "Let me try...", "I\'ll now...", "Let me check...", or promise to take any action',
		"- If you don't know the answer, say so - do not offer to look it up or investigate",
		"",
		"Simply answer the question with the information you have.",
		"</system-reminder>",
		"",
		question,
	].join("\n");
}

function resolveModel(ctx: ExtensionCommandContext, modelRef: string | undefined) {
	if (!modelRef || !modelRef.trim()) return undefined;
	const sessionModel = ctx.model;
	const raw = modelRef.trim();
	const canonicalSeparator = raw.indexOf("::");
	const slash = raw.indexOf("/");
	let provider: string | undefined;
	let id: string;
	if (canonicalSeparator > 0) {
		provider = raw.slice(0, canonicalSeparator);
		id = raw.slice(canonicalSeparator + 2);
	} else if (slash > 0) {
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
	const all = ctx.modelRegistry.getAll();
	return all.find(
		(m) =>
			m.id === id ||
			`${m.provider}/${m.id}` === raw ||
			`${m.provider}::${m.id}` === raw,
	);
}

function modelChain(parsed: BtwArgs, sessionModel: { provider?: string; id?: string } | undefined): string[] {
	const out: string[] = [];
	const push = (ref: string | undefined) => {
		const t = (ref ?? "").trim();
		if (!t) return;
		if (!out.includes(t)) out.push(t);
	};
	if (Array.isArray(parsed.models)) {
		for (const m of parsed.models) push(typeof m === "string" ? m : undefined);
	}
	push(parsed.model);
	if (out.length === 0 && sessionModel?.id) {
		const p = sessionModel.provider;
		push(p ? `${p}::${sessionModel.id}` : sessionModel.id);
	}
	return out;
}

export default function (pi: ExtensionAPI) {
	function emit(
		requestId: string,
		payload: {
			ok: boolean;
			answer?: string;
			error?: string;
			modelUsed?: string;
		},
	) {
		pi.sendMessage(
			{
				customType: BRIDGE_TYPE,
				content: payload.ok ? (payload.answer ?? "") : (payload.error ?? "error"),
				display: false,
				details: {
					version: 1,
					requestId,
					...payload,
				},
			},
			{ triggerTurn: false },
		);
	}

	pi.registerCommand(COMMAND, {
		description: "Internal Pi-Grok bridge: /btw side question",
		handler: async (args, ctx: ExtensionCommandContext) => {
			const parsed = parseArgs(args);
			const requestId = String(parsed.requestId ?? "").trim() || `btw-${Date.now()}`;
			const question = String(parsed.question ?? "").trim();
			if (!question) {
				emit(requestId, { ok: false, error: "Empty side question" });
				return;
			}

			try {
				const branch = ctx.sessionManager.getBranch() as Array<Record<string, unknown>>;
				const conversation = buildSideContext(branch);
				const chain = modelChain(parsed, ctx.model as { provider?: string; id?: string } | undefined);
				if (chain.length === 0) {
					emit(requestId, {
						ok: false,
						error: "No model available for /btw. Configure btw models in F2 or select a session model.",
					});
					return;
				}

				const userMessage: Message = {
					role: "user",
					content: [
						{
							type: "text",
							text: conversation
								? `${sideQuestionInstruction(question)}\n\n<conversation>\n${conversation}\n</conversation>`
								: sideQuestionInstruction(question),
						},
					],
					timestamp: Date.now(),
				};

				const errors: string[] = [];
				for (const modelRef of chain) {
					const model = resolveModel(ctx, modelRef);
					if (!model) {
						errors.push(`${modelRef}: not found`);
						continue;
					}
					const auth = await ctx.modelRegistry.getApiKeyAndHeaders(model);
					if (!auth.ok || !auth.apiKey) {
						errors.push(`${modelRef}: no API key`);
						continue;
					}
					try {
						const response = await complete(
							model,
							{ messages: [userMessage] },
							{
								apiKey: auth.apiKey,
								headers: auth.headers,
								env: auth.env,
								reasoning:
									model.reasoning && parsed.thinkingLevel && parsed.thinkingLevel !== "max"
										? parsed.thinkingLevel
										: model.reasoning && parsed.thinkingLevel === "max"
											? "xhigh"
											: undefined,
							},
						);
						if (response.stopReason === "aborted" || response.stopReason === "error") {
							errors.push(`${modelRef}: ${response.stopReason}`);
							continue;
						}
						const answer = (response.content ?? [])
							.filter((c): c is { type: "text"; text: string } => c.type === "text")
							.map((c) => c.text)
							.join("\n")
							.trim();
						if (!answer) {
							errors.push(`${modelRef}: empty response`);
							continue;
						}
						emit(requestId, {
							ok: true,
							answer,
							modelUsed: `${model.provider}::${model.id}`,
						});
						return;
					} catch (e) {
						errors.push(`${modelRef}: ${e instanceof Error ? e.message : String(e)}`);
					}
				}

				emit(requestId, {
					ok: false,
					error: `All /btw models failed. Reconfigure F2 btw models. (${errors.join("; ")})`,
				});
			} catch (e) {
				emit(requestId, {
					ok: false,
					error: e instanceof Error ? e.message : String(e),
				});
			}
		},
	});
}
