/**
 * Default-on Pi auth for grok-pi (min Pi 0.80.10).
 *
 * Mirrors interactive-mode /login flow:
 * 1) Select authentication method (account vs API key)
 * 2) Select provider for that method
 * 3) LoginDialog + modelRuntime.login(...)
 *
 * Remote TUI: extension monkey-patch of ctx.ui.custom (RPC stub alone is not enough).
 */

import * as path from "node:path";
import { realpathSync } from "node:fs";
import { pathToFileURL } from "node:url";
import type { ExtensionAPI, ExtensionCommandContext } from "@earendil-works/pi-coding-agent";
import type { Component, TUI } from "@earendil-works/pi-tui";

type AuthType = "oauth" | "api_key";

type AuthMethod = {
	name?: string;
	loginLabel?: string;
	login?: unknown;
};

type ProviderOption = {
	id: string;
	name: string;
	authType: AuthType;
	method?: AuthMethod;
	status?: { type: AuthType; source?: string };
};

type ModelRuntimeLike = {
	getProviders: () => Array<{
		id: string;
		name: string;
		auth?: { oauth?: AuthMethod; apiKey?: AuthMethod };
	}>;
	getProvider?: (id: string) => { name?: string } | undefined;
	getProviderAuthStatus: (id: string) => {
		configured?: boolean;
		source?: string;
		label?: string;
	};
	isUsingOAuth?: (id: string) => boolean;
	listCredentials:
		| (() => Promise<Array<{ providerId: string; type: AuthType }>>)
		| (() => Array<{ providerId: string; type: AuthType }>);
	login: (
		providerId: string,
		method: AuthType,
		interaction: {
			signal?: AbortSignal;
			prompt: (prompt: AuthPrompt) => Promise<string>;
			notify: (event: AuthNotify) => void;
		},
	) => Promise<unknown>;
	logout: (providerId: string) => Promise<void>;
	getAvailable?: () => Promise<unknown> | unknown;
	refresh?: (options?: unknown) => Promise<unknown>;
};

type AuthPrompt = { signal?: AbortSignal } &
	(
		| { type: "text"; message: string; placeholder?: string }
		| { type: "secret"; message: string; placeholder?: string }
		| { type: "manual_code"; message: string; placeholder?: string }
		| { type: "select"; message: string; options: Array<{ id: string; label: string }> }
	);

type AuthNotify =
	| { type: "auth_url"; url: string; instructions?: string }
	| { type: "device_code"; userCode?: string; verificationUri?: string; [k: string]: unknown }
	| { type: "info"; message: string; links?: unknown[] }
	| { type: "progress"; message: string };

type ModelRegistryLike = {
	runtime?: ModelRuntimeLike;
	getProviderDisplayName?: (id: string) => string;
	refresh?: () => unknown;
};

interface OAuthSelectorConstructor {
	new (
		mode: "login" | "logout",
		providers: ProviderOption[],
		onSelect: (providerId: string, authType: AuthType) => void,
		onCancel: () => void,
		initialSearchInput?: string,
	): Component;
}

type LoginDialog = Component & {
	signal: AbortSignal;
	showAuth(url: string, instructions?: string): void;
	showDeviceCode(info: unknown): void;
	showPrompt(message: string, placeholder?: string): Promise<string>;
	showManualInput(prompt: string): Promise<string>;
	showProgress(message: string): void;
	showWaiting(message: string): void;
	showInfo?(message: string, links?: unknown[], showCloseHint?: boolean): void;
	showDetails?(lines: string[]): void;
};

interface LoginDialogConstructor {
	new (
		tui: TUI,
		providerId: string,
		onComplete: (success: boolean, message?: string) => void,
		providerNameOverride?: string,
		titleOverride?: string,
	): LoginDialog;
}

type OverlayHandle = {
	hide: () => void;
	show?: () => void;
	setVisible?: (visible: boolean) => void;
};

type AuthTui = TUI & {
	showOverlay?: (component: Component) => OverlayHandle;
	setFocus?: (component: Component | null) => void;
};

interface ExtensionSelectorConstructor {
	new (
		title: string,
		options: string[],
		onSelect: (option: string) => void,
		onCancel: () => void,
	): Component;
}

function hostUrl(relativePath: string): string {
	const hostDistDir = path.dirname(realpathSync(process.argv[1]!));
	return new URL(relativePath, pathToFileURL(hostDistDir).href + "/").href;
}

function ensureRemoteTuiHost(ui: ExtensionCommandContext["ui"]): void {
	const ensure = (
		globalThis as typeof globalThis & {
			__piGrokEnsureRemoteTuiHost?: (ui: ExtensionCommandContext["ui"]) => void;
		}
	).__piGrokEnsureRemoteTuiHost;
	if (typeof ensure === "function") ensure(ui);
}

async function ensurePiTheme(): Promise<void> {
	const mod = (await import(hostUrl("modes/interactive/theme/theme.js"))) as {
		theme?: { name?: string };
		initTheme?: (name?: string, enableWatcher?: boolean) => void;
	};
	try {
		void mod.theme?.name;
	} catch {
		mod.initTheme?.(undefined, false);
		void mod.theme?.name;
	}
}

function resolveRuntime(ctx: ExtensionCommandContext): ModelRuntimeLike {
	const registry = ctx.modelRegistry as unknown as ModelRegistryLike | undefined;
	const runtime = registry?.runtime;
	if (!runtime || typeof runtime.login !== "function" || typeof runtime.getProviders !== "function") {
		throw new Error(
			"Pi ModelRuntime unavailable on ctx.modelRegistry.runtime. " +
				"grok-pi requires Pi >= 0.80.10 (system `pi`).",
		);
	}
	return runtime;
}

async function loadComponents(): Promise<{
	OAuthSelectorComponent: OAuthSelectorConstructor;
	LoginDialogComponent: LoginDialogConstructor;
	ExtensionSelectorComponent: ExtensionSelectorConstructor;
}> {
	const [oauth, login, selector] = await Promise.all([
		import(hostUrl("modes/interactive/components/oauth-selector.js")) as Promise<{
			OAuthSelectorComponent: OAuthSelectorConstructor;
		}>,
		import(hostUrl("modes/interactive/components/login-dialog.js")) as Promise<{
			LoginDialogComponent: LoginDialogConstructor;
		}>,
		import(hostUrl("modes/interactive/components/extension-selector.js")) as Promise<{
			ExtensionSelectorComponent: ExtensionSelectorConstructor;
		}>,
	]);
	return {
		OAuthSelectorComponent: oauth.OAuthSelectorComponent,
		LoginDialogComponent: login.LoginDialogComponent,
		ExtensionSelectorComponent: selector.ExtensionSelectorComponent,
	};
}

function loginProviders(runtime: ModelRuntimeLike, authType?: AuthType): ProviderOption[] {
	const options: ProviderOption[] = [];
	for (const provider of runtime.getProviders()) {
		const authStatus = runtime.getProviderAuthStatus(provider.id);
		const status = authStatus?.configured
			? {
					type: (runtime.isUsingOAuth?.(provider.id) ? "oauth" : "api_key") as AuthType,
					source: authStatus.label ?? authStatus.source,
				}
			: undefined;

		if ((!authType || authType === "oauth") && provider.auth?.oauth) {
			options.push({
				id: provider.id,
				name: provider.name,
				authType: "oauth",
				method: provider.auth.oauth,
				status,
			});
		}
		if ((!authType || authType === "api_key") && provider.auth?.apiKey) {
			options.push({
				id: provider.id,
				name: provider.name,
				authType: "api_key",
				method: provider.auth.apiKey,
				status,
			});
		}
	}
	return options.sort((a, b) => a.name.localeCompare(b.name));
}

function findLoginProviderOptions(runtime: ModelRuntimeLike, providerRef: string): ProviderOption[] {
	const query = providerRef.trim().toLowerCase();
	if (!query) return loginProviders(runtime);
	return loginProviders(runtime).filter(
		(p) => p.id.toLowerCase() === query || p.name.toLowerCase() === query,
	);
}

async function logoutProviders(runtime: ModelRuntimeLike): Promise<ProviderOption[]> {
	const credentials = await runtime.listCredentials();
	return credentials
		.map(({ providerId, type }) => ({
			id: providerId,
			name: runtime.getProvider?.(providerId)?.name ?? providerId,
			authType: type,
			status: { type, source: "stored credential" },
		}))
		.sort((a, b) => a.name.localeCompare(b.name));
}

async function openCustom<T>(
	ctx: ExtensionCommandContext,
	factory: (tui: TUI, theme: unknown, kb: unknown, done: (value: T) => void) => Component,
): Promise<{ ran: boolean; value: T | undefined }> {
	let ran = false;
	const value = await ctx.ui.custom<T>((tui, theme, kb, done) => {
		ran = true;
		return factory(tui as TUI, theme, kb, done);
	});
	return { ran, value };
}

async function prepareUi(ctx: ExtensionCommandContext, command: string): Promise<boolean> {
	if (process.env.PI_GROK_REMOTE_TUI !== "1") {
		ctx.ui.notify(
			`/${command} needs PI_GROK_REMOTE_TUI=1 (Remote TUI). Restart grok-pi without PI_GROK_REMOTE_TUI=0.`,
			"error",
		);
		return false;
	}
	ensureRemoteTuiHost(ctx.ui);
	try {
		await ensurePiTheme();
	} catch (error: unknown) {
		ctx.ui.notify(
			`/${command}: theme init failed: ${error instanceof Error ? error.message : String(error)}`,
			"error",
		);
		return false;
	}
	return true;
}

function oauthLabelFor(options?: ProviderOption[]): string {
	const oauth = options?.find((p) => p.authType === "oauth");
	const custom = oauth?.method?.loginLabel;
	return typeof custom === "string" && custom.trim() ? custom : "Sign in with an account";
}

export default function piGrokAuth(pi: ExtensionAPI) {
	if (process.env.PI_GROK !== "1") return;

	pi.registerCommand("login", {
		description: "Log in to a Pi model provider",
		handler: async (args: string, ctx: ExtensionCommandContext) => {
			try {
				if (!(await prepareUi(ctx, "login"))) return;

				const runtime = resolveRuntime(ctx);
				const registry = ctx.modelRegistry as unknown as ModelRegistryLike;
				const { OAuthSelectorComponent, LoginDialogComponent, ExtensionSelectorComponent } =
					await loadComponents();

				await runtime.getAvailable?.();

				const selectOption = async (
					tui: AuthTui,
					dialog: LoginDialog,
					prompt: Extract<AuthPrompt, { type: "select" }>,
				): Promise<string> => {
					const labels = prompt.options.map((o) => o.label);
					// Nested openCustom tears down LoginDialog (and its callback listener UI).
					// Overlay on the same remote-tui host, matching interactive-mode.
					if (typeof tui.showOverlay !== "function") {
						throw new Error("Remote TUI showOverlay unavailable for login select");
					}
					return new Promise<string>((resolve, reject) => {
						let settled = false;
						let overlay: OverlayHandle | undefined;
						const finish = (fn: () => void) => {
							if (settled) return;
							settled = true;
							try {
								overlay?.hide();
							} catch {
								/* ignore */
							}
							tui.setFocus?.(dialog);
							fn();
						};
						const selector = new ExtensionSelectorComponent(
							prompt.message,
							labels,
							(optionLabel) => {
								const id = prompt.options.find((o) => o.label === optionLabel)?.id;
								if (id) finish(() => resolve(id));
								else finish(() => reject(new Error("Login cancelled")));
							},
							() => finish(() => reject(new Error("Login cancelled"))),
						);
						overlay = tui.showOverlay!(selector);
						tui.setFocus?.(selector);
					});
				};

				const showAuthPrompt = async (
					tui: AuthTui,
					dialog: LoginDialog,
					prompt: AuthPrompt,
				): Promise<string> => {
					let response: Promise<string>;
					if (prompt.type === "select") {
						response = selectOption(tui, dialog, prompt);
					} else if (prompt.type === "manual_code") {
						response = dialog.showManualInput(prompt.message);
					} else {
						response = dialog.showPrompt(prompt.message, prompt.placeholder);
					}
					// Race prompt.signal so callback-server wins can cancel manual paste.
					if (!prompt.signal) return response;
					if (prompt.signal.aborted) throw new Error("Login cancelled");
					const signal = prompt.signal;
					let onAbort: (() => void) | undefined;
					const aborted = new Promise<string>((_resolve, reject) => {
						onAbort = () => reject(new Error("Login cancelled"));
						signal.addEventListener("abort", onAbort, { once: true });
					});
					try {
						return await Promise.race([response, aborted]);
					} finally {
						if (onAbort) signal.removeEventListener("abort", onAbort);
					}
				};

				const authenticate = async (provider: ProviderOption) => {
					await ensurePiTheme();
					ensureRemoteTuiHost(ctx.ui);

					// Ambient/non-login methods: info only (matches interactive-mode).
					if (provider.authType === "api_key" && provider.method && !provider.method.login) {
						const { ran } = await openCustom<void>(ctx, (tui, _theme, _kb, done) => {
							const dialog = new LoginDialogComponent(
								tui,
								provider.id,
								() => done(undefined),
								provider.name,
								`${provider.name} setup`,
							);
							dialog.showInfo?.(
								`${provider.method?.name ?? "Authentication"} is configured outside pi.`,
								[],
								true,
							);
							return dialog;
						});
						if (!ran) {
							ctx.ui.notify("/login: Remote TUI custom() unavailable. Try /remote-tui.", "error");
						}
						return;
					}

					let loginError: string | undefined;
					const { ran, value: success } = await openCustom<boolean>(ctx, (tui, _theme, _kb, done) => {
						const authTui = tui as AuthTui;
						const dialog = new LoginDialogComponent(
							tui,
							provider.id,
							() => done(false),
							provider.name,
						);

						void (async () => {
							try {
								await runtime.login(provider.id, provider.authType, {
									signal: dialog.signal,
									prompt: (prompt) => showAuthPrompt(authTui, dialog, prompt),
									notify: (event) => {
										if (event.type === "auth_url") {
											dialog.showAuth(event.url, event.instructions);
										} else if (event.type === "device_code") {
											dialog.showDeviceCode(event);
											dialog.showWaiting("Waiting for authentication...");
										} else if (event.type === "info") {
											dialog.showInfo?.(event.message, event.links);
										} else if (event.type === "progress") {
											dialog.showProgress(event.message);
										}
									},
								});
								await registry.refresh?.();
								await runtime.getAvailable?.();
								done(true);
							} catch (error: unknown) {
								const message = error instanceof Error ? error.message : String(error);
								if (message !== "Login cancelled") loginError = message;
								done(false);
							}
						})();

						return dialog;
					});

					if (!ran) {
						ctx.ui.notify("/login: Remote TUI custom() unavailable. Try /remote-tui.", "error");
						return;
					}

					// Defer notify one tick so Pager finishes processing the widget
					// teardown frame before rendering the toast (same-frame toasts get swallowed).
					await new Promise<void>((r) => setTimeout(r, 60));
					if (loginError) {
						ctx.ui.notify(`Login failed: ${loginError}`, "error");
					} else if (success) {
						const msg =
							provider.authType === "oauth"
								? `Logged in to ${provider.name}`
								: `Saved API key for ${provider.name}`;
						ctx.ui.notify(msg, "success");
					}
				};

				const showProviderSelector = async (
					authType: AuthType | undefined,
					initialSearch?: string,
					onCancelBackToAuthType = false,
				) => {
					const providers = loginProviders(runtime, authType);
					if (providers.length === 0) {
						const message =
							authType === "oauth"
								? "No subscription providers available."
								: authType === "api_key"
									? "No API key providers available."
									: "No login providers available.";
						ctx.ui.notify(message, "warning");
						return;
					}

					const { ran } = await openCustom<void>(ctx, (_tui, _theme, _kb, done) => {
						return new OAuthSelectorComponent(
							"login",
							providers,
							(providerId, selectedAuthType) => {
								done(undefined);
								const provider = providers.find(
									(p) => p.id === providerId && p.authType === selectedAuthType,
								);
								if (provider) void authenticate(provider);
							},
							() => {
								done(undefined);
								if (onCancelBackToAuthType) {
									void showAuthTypeSelector();
								}
							},
							initialSearch,
						);
					});
					if (!ran) {
						ctx.ui.notify("/login: Remote TUI custom() unavailable. Try /remote-tui.", "error");
					}
				};

				const showAuthTypeSelector = async (scopedProviders?: ProviderOption[]) => {
					const subscriptionLabel = oauthLabelFor(scopedProviders);
					const apiKeyLabel = "Sign in with an API key";
					const available = scopedProviders
						? new Set(scopedProviders.map((p) => p.authType))
						: new Set<AuthType>(["oauth", "api_key"]);

					const options: string[] = [];
					if (available.has("oauth")) options.push(subscriptionLabel);
					if (available.has("api_key")) options.push(apiKeyLabel);

					if (options.length === 0) {
						ctx.ui.notify("No login methods available.", "warning");
						return;
					}

					// Single method for a scoped provider → go straight to login.
					if (scopedProviders && options.length === 1) {
						const only = scopedProviders[0];
						if (only) await authenticate(only);
						return;
					}

					const title = scopedProviders?.[0]
						? `Select authentication method for ${scopedProviders[0].name}:`
						: "Select authentication method:";

					const { ran, value } = await openCustom<string | undefined>(
						ctx,
						(_tui, _theme, _kb, done) =>
							new ExtensionSelectorComponent(title, options, done, () => done(undefined)),
					);
					if (!ran) {
						ctx.ui.notify("/login: Remote TUI custom() unavailable. Try /remote-tui.", "error");
						return;
					}
					if (!value) return;

					const authType: AuthType = value === subscriptionLabel ? "oauth" : "api_key";
					if (scopedProviders) {
						const provider = scopedProviders.find((p) => p.authType === authType);
						if (provider) await authenticate(provider);
						return;
					}
					await showProviderSelector(authType, undefined, true);
				};

				// --- match interactive-mode handleLoginCommand ---
				const providerRef = args.trim();
				if (!providerRef) {
					await showAuthTypeSelector();
					return;
				}

				const matches = findLoginProviderOptions(runtime, providerRef);
				if (matches.length === 1) {
					await authenticate(matches[0]!);
					return;
				}
				if (matches.length > 1) {
					const providerIds = new Set(matches.map((p) => p.id));
					if (providerIds.size === 1) {
						// Same provider, multiple auth methods → method picker.
						await showAuthTypeSelector(matches);
						return;
					}
				}
				if (matches.length === 0) {
					ctx.ui.notify(`No login provider matching "${providerRef}"`, "warning");
					return;
				}
				// Ambiguous ref across providers → provider list pre-filtered by search.
				await showProviderSelector(undefined, providerRef, false);
			} catch (error: unknown) {
				ctx.ui.notify(`/login failed: ${error instanceof Error ? error.message : String(error)}`, "error");
			}
		},
	});

	pi.registerCommand("logout", {
		description: "Remove a stored Pi provider credential",
		handler: async (_args: string, ctx: ExtensionCommandContext) => {
			try {
				if (!(await prepareUi(ctx, "logout"))) return;

				const runtime = resolveRuntime(ctx);
				const registry = ctx.modelRegistry as unknown as ModelRegistryLike;
				const { OAuthSelectorComponent } = await loadComponents();
				const providers = await logoutProviders(runtime);

				if (providers.length === 0) {
					ctx.ui.notify(
						"No credentials saved by /login. Environment variables and models.json are unchanged.",
						"info",
					);
					return;
				}

				const { ran } = await openCustom<void>(ctx, (_tui, _theme, _kb, done) => {
					return new OAuthSelectorComponent(
						"logout",
						providers,
						(providerId) => {
							const provider = providers.find((p) => p.id === providerId);
							if (!provider) return;
							void (async () => {
								try {
									await runtime.logout(providerId);
									await registry.refresh?.();
									done(undefined);
									ctx.ui.notify(`Logged out of ${provider.name}`, "success");
								} catch (error: unknown) {
									done(undefined);
									ctx.ui.notify(
										`Logout failed: ${error instanceof Error ? error.message : String(error)}`,
										"error",
									);
								}
							})();
						},
						() => done(undefined),
					);
				});

				if (!ran) {
					ctx.ui.notify("/logout: Remote TUI custom() unavailable. Try /remote-tui.", "error");
				}
			} catch (error: unknown) {
				ctx.ui.notify(`/logout failed: ${error instanceof Error ? error.message : String(error)}`, "error");
			}
		},
	});
}
