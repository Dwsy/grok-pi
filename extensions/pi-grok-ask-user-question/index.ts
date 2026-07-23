/**
 * Native Grok Q&A for grok-pi.
 *
 * Registers `ask_user_question` so the model can open Grok's QuestionView.
 * The adapter opens `x.ai/ask_user_question` on tool_execution_start and writes
 * the result into PI_GROK_ASK_USER_DIR/<toolCallId>.json; this extension waits
 * for that file and returns the model-facing tool result.
 *
 * F2 `[ui].pi_ask_user_question` (default off) gates injection at startup.
 */
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { Type } from "@sinclair/typebox";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

const CANCEL_TEXT =
	"User declined to answer the questions. Continue with the task using your best judgment, or ask different questions.";

const OptionSchema = Type.Object({
	label: Type.String({ description: "Option text shown to the user. A few words at most." }),
	description: Type.String({ description: "What picking this option means or implies." }),
	preview: Type.Optional(
		Type.String({
			description:
				"Optional content shown while the option is focused. Single-select questions only.",
		}),
	),
});

const QuestionSchema = Type.Object({
	question: Type.String({ description: "The question to ask, phrased as a full question." }),
	header: Type.Optional(Type.String({ description: "Short chip/tag next to the question (max ~16 chars)." })),
	options: Type.Array(OptionSchema, { minItems: 2, maxItems: 4 }),
	multi_select: Type.Optional(
		Type.Boolean({ description: "Let the user pick more than one option (default false)." }),
	),
});

const Parameters = Type.Object({
	questions: Type.Array(QuestionSchema, {
		minItems: 1,
		maxItems: 4,
		description: "The questions to ask, each with its own options.",
	}),
});

type ResponseFile =
	| { outcome: "accepted"; message: string }
	| { outcome: "cancelled"; message?: string }
	| { outcome: "error"; message: string };

function controlDir(): string | undefined {
	const value = process.env.PI_GROK_ASK_USER_DIR?.trim();
	return value || undefined;
}

function sleep(ms: number, signal?: AbortSignal): Promise<void> {
	return new Promise((resolve, reject) => {
		if (signal?.aborted) {
			reject(new Error("aborted"));
			return;
		}
		const timer = setTimeout(() => {
			signal?.removeEventListener("abort", onAbort);
			resolve();
		}, ms);
		const onAbort = () => {
			clearTimeout(timer);
			reject(new Error("aborted"));
		};
		signal?.addEventListener("abort", onAbort, { once: true });
	});
}

async function waitForResponse(toolCallId: string, signal?: AbortSignal): Promise<ResponseFile> {
	const dir = controlDir();
	if (!dir) {
		return {
			outcome: "error",
			message: "ask_user_question is unavailable (host control missing). Enable F2 Pi Q&A and restart grok-pi.",
		};
	}
	const path = join(dir, `${toolCallId}.json`);
	// Adapter opens the native overlay on tool_start; poll until it writes.
	const deadline = Date.now() + 30 * 60 * 1000;
	while (Date.now() < deadline) {
		if (signal?.aborted) throw new Error("aborted");
		if (existsSync(path)) {
			try {
				const raw = readFileSync(path, "utf8");
				const parsed = JSON.parse(raw) as ResponseFile;
				if (parsed && typeof parsed === "object" && "outcome" in parsed) {
					return parsed;
				}
			} catch {
				// Partial write race — retry.
			}
		}
		await sleep(40, signal);
	}
	return { outcome: "cancelled", message: CANCEL_TEXT };
}

export default function (pi: ExtensionAPI) {
	pi.registerTool({
		name: "ask_user_question",
		label: "Q&A",
		description: `Ask the user one or more multiple-choice questions.

Grok Build asks the right questions to nail the details.

- Every question automatically gets an "Other" choice where the user can type their own answer.
- Put your recommended option first and append "(Recommended)" to its label.
- Prefer this tool when requirements are ambiguous instead of guessing.`,
		promptSnippet: "Ask the user structured multiple-choice questions via the native Q&A overlay.",
		parameters: Parameters,
		async execute(toolCallId, _params, signal) {
			const result = await waitForResponse(toolCallId, signal);
			if (result.outcome === "accepted") {
				return {
					content: [{ type: "text", text: result.message }],
					details: { outcome: "accepted" },
				};
			}
			if (result.outcome === "error") {
				return {
					content: [{ type: "text", text: result.message }],
					details: { outcome: "error" },
				};
			}
			return {
				content: [{ type: "text", text: result.message ?? CANCEL_TEXT }],
				details: { outcome: "cancelled" },
			};
		},
	});
}
