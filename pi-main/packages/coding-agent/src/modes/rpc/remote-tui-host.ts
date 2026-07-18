/**
 * Experimental Remote TUI host for RPC mode.
 *
 * When PI_GROK_REMOTE_TUI=1, ctx.ui.custom(factory) runs the component inside
 * the Pi process and projects serializable frames to the host (Grok) over JSONL.
 * Default remains a no-op stub.
 *
 * Component objects never cross the process boundary — only lines[] and key data.
 */

import type { Component } from "@earendil-works/pi-tui";
import { isFocusable, setKeybindings } from "@earendil-works/pi-tui";
import { KeybindingsManager } from "../../core/keybindings.ts";
import type { Theme } from "../interactive/theme/theme.ts";
import { initTheme, theme } from "../interactive/theme/theme.ts";

export type RemoteTuiEmitter = (request: Record<string, unknown>) => void;

export type RemoteTuiFactory<T> = (
	tui: RemoteTuiStub,
	theme: Theme,
	keybindings: KeybindingsManager,
	done: (result: T) => void,
) => (Component & { dispose?(): void }) | Promise<Component & { dispose?(): void }>;

export type RemoteTuiOptions = {
	overlay?: boolean;
	overlayOptions?: unknown;
	onHandle?: (handle: { hide: () => void; show: () => void }) => void;
};

/** Minimal TUI surface so factory code can call requestRender/setFocus. */
export type RemoteTuiStub = {
	requestRender: (force?: boolean) => void;
	setFocus: (component: Component | null) => void;
	showOverlay: (
		component: Component,
		_options?: unknown,
	) => { hide: () => void; show: () => void; setVisible: (v: boolean) => void };
	hideOverlay: () => void;
	addChild: (_component: Component) => void;
	removeChild: (_component: Component) => void;
};

type ActiveSession = {
	id: string;
	component: Component & { dispose?(): void };
	closed: boolean;
	width: number;
	pushFrame: () => void;
	handleInput: (data: string) => void;
	close: (result: unknown) => void;
};

const activeById = new Map<string, ActiveSession>();

export function isRemoteTuiEnabled(): boolean {
	return process.env.PI_GROK_REMOTE_TUI === "1";
}

export function handleRemoteTuiInput(id: string, data: string): boolean {
	const session = activeById.get(id);
	if (!session || session.closed) return false;
	session.handleInput(data);
	return true;
}

export function handleRemoteTuiCancel(id: string): boolean {
	const session = activeById.get(id);
	if (!session || session.closed) return false;
	session.close(undefined);
	return true;
}

export function hasActiveRemoteTui(): boolean {
	for (const session of activeById.values()) {
		if (!session.closed) return true;
	}
	return false;
}

/**
 * Run a custom component factory under the experimental remote host.
 * Resolves when the factory calls done(result) or cancel arrives.
 */
export async function runRemoteCustom<T>(
	emit: RemoteTuiEmitter,
	factory: RemoteTuiFactory<T>,
	_options?: RemoteTuiOptions,
): Promise<T> {
	const id = crypto.randomUUID();
	const width = Number(process.env.PI_GROK_REMOTE_TUI_WIDTH) || 72;
	// RPC already calls initTheme in main, but ensure proxy is live if host is used alone.
	try {
		void theme.name;
	} catch {
		initTheme(undefined, false);
	}
	const keybindings = KeybindingsManager.create();
	setKeybindings(keybindings);

	return new Promise<T>((resolve, reject) => {
		let component: (Component & { dispose?(): void }) | undefined;
		let closed = false;
		let focused: Component | null = null;

		const cleanup = () => {
			activeById.delete(id);
			try {
				component?.dispose?.();
			} catch {
				/* ignore */
			}
		};

		const close = (result: unknown) => {
			if (closed) return;
			closed = true;
			emit({
				type: "extension_ui_request",
				id,
				method: "remote_tui_close",
			});
			cleanup();
			resolve(result as T);
		};

		const pushFrame = () => {
			if (closed || !component) return;
			try {
				const lines = component.render(width);
				emit({
					type: "extension_ui_request",
					id,
					method: "remote_tui_frame",
					lines: Array.isArray(lines) ? lines.map(String) : [],
					width,
				});
			} catch (error) {
				if (closed) return;
				closed = true;
				emit({
					type: "extension_ui_request",
					id,
					method: "remote_tui_close",
				});
				cleanup();
				reject(error instanceof Error ? error : new Error(String(error)));
			}
		};

		const handleInput = (data: string) => {
			if (closed) return;
			const target = focused ?? component;
			if (target?.handleInput) {
				try {
					target.handleInput(data);
				} catch (error) {
					if (closed) return;
					closed = true;
					emit({
						type: "extension_ui_request",
						id,
						method: "remote_tui_close",
					});
					cleanup();
					reject(error instanceof Error ? error : new Error(String(error)));
					return;
				}
			}
			// Many components mutate state in handleInput but rely on TUI requestRender.
			pushFrame();
		};

		const tui: RemoteTuiStub = {
			requestRender: () => {
				// Defer so done()/state updates settle before snapshot.
				process.nextTick(() => {
					if (!closed) pushFrame();
				});
			},
			setFocus: (next) => {
				if (isFocusable(focused)) {
					focused.focused = false;
				}
				focused = next;
				if (isFocusable(next)) {
					next.focused = true;
				}
			},
			showOverlay: (overlayComponent) => {
				// L1: treat overlay as the focused component surface.
				component = overlayComponent as Component & { dispose?(): void };
				tui.setFocus(overlayComponent);
				pushFrame();
				return {
					hide: () => {},
					show: () => pushFrame(),
					setVisible: () => {},
				};
			},
			hideOverlay: () => {},
			addChild: () => {},
			removeChild: () => {},
		};

		emit({
			type: "extension_ui_request",
			id,
			method: "remote_tui_open",
			title: "Remote TUI",
			width,
		});

		const session: ActiveSession = {
			id,
			component: undefined as unknown as Component & { dispose?(): void },
			closed: false,
			width,
			pushFrame,
			handleInput,
			close,
		};
		// session.component assigned after factory resolves
		activeById.set(id, session);

		Promise.resolve(factory(tui, theme, keybindings, close as (result: T) => void))
			.then((created) => {
				if (closed) {
					try {
						created.dispose?.();
					} catch {
						/* ignore */
					}
					return;
				}
				component = created;
				session.component = created;
				tui.setFocus(created);
				pushFrame();
			})
			.catch((error) => {
				if (closed) return;
				closed = true;
				emit({
					type: "extension_ui_request",
					id,
					method: "remote_tui_close",
				});
				cleanup();
				reject(error);
			});
	});
}
