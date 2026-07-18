import * as path from "node:path";
import { realpathSync } from "node:fs";
import { pathToFileURL } from "node:url";
import type { ExtensionAPI, ExtensionCommandContext } from "@earendil-works/pi-coding-agent";
import { getProviders, type Model, type OAuthProviderId, type OAuthSelectPrompt } from "@earendil-works/pi-ai";
import type { Component, TUI } from "@earendil-works/pi-tui";

type SessionListProgress = (loaded: number, total: number) => void;

let builtInApiKeyProviders = new Set<string>();

type SessionInfo = {
	path: string;
	name?: string;
};

interface ModelSelectorConstructor {
	new (
		tui: TUI,
		currentModel: Model<any> | undefined,
		settingsManager: { setDefaultModelAndProvider(provider: string, modelId: string): void },
		modelRegistry: ExtensionCommandContext["modelRegistry"],
		scopedModels: readonly [],
		onSelect: (model: Model<any>) => void,
		onCancel: () => void,
		initialSearchInput?: string,
	): Component;
}

interface SessionSelectorConstructor {
	new (
		currentSessionsLoader: (onProgress?: SessionListProgress) => Promise<SessionInfo[]>,
		allSessionsLoader: (onProgress?: SessionListProgress) => Promise<SessionInfo[]>,
		onSelect: (sessionPath: string) => void,
		onCancel: () => void,
		onExit: () => void,
		requestRender: () => void,
		options: { showRenameHint: boolean },
		currentSessionFilePath?: string,
	): Component;
}

interface SessionManagerStatic {
	list(cwd: string, sessionDir: string | undefined, onProgress?: SessionListProgress): Promise<SessionInfo[]>;
	listAll(sessionDir?: string, onProgress?: SessionListProgress): Promise<SessionInfo[]>;
}

interface SettingsManagerStatic {
	create(cwd: string): { setDefaultModelAndProvider(provider: string, modelId: string): void };
}

type AuthSelectorProvider = {
	id: string;
	name: string;
	authType: "oauth" | "api_key";
};

interface OAuthSelectorConstructor {
	new (
		mode: "login" | "logout",
		authStorage: ExtensionCommandContext["modelRegistry"]["authStorage"],
		providers: AuthSelectorProvider[],
		onSelect: (providerId: string, authType: AuthSelectorProvider["authType"]) => void,
		onCancel: () => void,
		getAuthStatus?: (providerId: string) => unknown,
		initialSearchInput?: string,
	): Component;
}

interface LoginDialogConstructor {
	new (
		tui: TUI,
		providerId: string,
		onComplete: (success: boolean, message?: string) => void,
		providerNameOverride?: string,
	): Component & {
		signal: AbortSignal;
		showAuth(url: string, instructions?: string): void;
		showDeviceCode(info: unknown): void;
		showPrompt(message: string, placeholder?: string): Promise<string>;
		showManualInput(prompt: string): Promise<string>;
		showProgress(message: string): void;
		showWaiting(message: string): void;
	};
}

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

function remoteTuiAvailable(): boolean {
	return process.env.PI_GROK_REMOTE_TUI === "1";
}

function unavailableCommand(name: string, reason: string) {
	return {
		description: `[experimental] Pi native /${name} (${reason})`,
		handler: async (_args: string, ctx: ExtensionCommandContext) => {
			ctx.ui.notify(`/${name} is not exposed by Pi RPC: ${reason}`, "warning");
		},
	};
}

function loginProviders(ctx: ExtensionCommandContext): AuthSelectorProvider[] {
	const oauthProviders = ctx.modelRegistry.authStorage.getOAuthProviders();
	const oauthIds = new Set(oauthProviders.map((provider) => provider.id));
	const providers = oauthProviders.map((provider) => ({
		id: provider.id,
		name: provider.name,
		authType: "oauth" as const,
	}));

	const builtInProviders = new Set<string>(getProviders());
	for (const providerId of new Set(ctx.modelRegistry.getAll().map((model) => model.provider))) {
		const supportsApiKey = builtInApiKeyProviders.has(providerId) ||
			(!oauthIds.has(providerId) && !builtInProviders.has(providerId));
		if (!supportsApiKey) continue;
		providers.push({
			id: providerId,
			name: ctx.modelRegistry.getProviderDisplayName(providerId),
			authType: "api_key",
		});
	}

	return providers.sort((left, right) => left.name.localeCompare(right.name));
}

function logoutProviders(ctx: ExtensionCommandContext): AuthSelectorProvider[] {
	return ctx.modelRegistry.authStorage
		.list()
		.flatMap((providerId) => {
			const credential = ctx.modelRegistry.authStorage.get(providerId);
			return credential
				? [{
						id: providerId,
						name: ctx.modelRegistry.getProviderDisplayName(providerId),
						authType: credential.type,
					}]
				: [];
		})
		.sort((left, right) => left.name.localeCompare(right.name));
}

export default async function piGrokNativeCommands(pi: ExtensionAPI) {
	if (process.env.PI_GROK !== "1") {
		return;
	}

	const [{ ModelSelectorComponent }, { SessionSelectorComponent, SessionManager, SettingsManager }, { BUILT_IN_PROVIDER_DISPLAY_NAMES }] = await Promise.all([
		import(hostUrl("modes/interactive/components/model-selector.js")) as Promise<{
			ModelSelectorComponent: ModelSelectorConstructor;
		}>,
		Promise.all([
			import(hostUrl("modes/interactive/components/session-selector.js")),
			import(hostUrl("core/session-manager.js")),
			import(hostUrl("core/settings-manager.js")),
		]).then(([selector, manager, settings]) => ({
			SessionSelectorComponent: selector.SessionSelectorComponent as SessionSelectorConstructor,
			SessionManager: manager.SessionManager as SessionManagerStatic,
			SettingsManager: settings.SettingsManager as SettingsManagerStatic,
		})),
		import(hostUrl("core/provider-display-names.js")) as Promise<{
			BUILT_IN_PROVIDER_DISPLAY_NAMES: Record<string, string>;
		}>,
	]);
	builtInApiKeyProviders = new Set(Object.keys(BUILT_IN_PROVIDER_DISPLAY_NAMES));

	pi.registerCommand("pi-model", {
		description: "[experimental] Open Pi's native model selector",
		handler: async (args: string, ctx: ExtensionCommandContext) => {
			if (!remoteTuiAvailable()) {
				ctx.ui.notify("/pi-model requires PI_GROK_REMOTE_TUI=1", "warning");
				return;
			}

			await ctx.ui.custom<void>((tui, _theme, _keybindings, done) => {
				const selector = new ModelSelectorComponent(
					tui,
					ctx.model,
					SettingsManager.create(ctx.cwd),
					ctx.modelRegistry,
					[],
					(model) => {
						done(undefined);
						void pi.setModel(model);
					},
					() => done(undefined),
					args.trim() || undefined,
				);
				return selector;
			});
		},
	});

	pi.registerCommand("pi-resume", {
		description: "[experimental] Open Pi's native session selector",
		handler: async (_args: string, ctx: ExtensionCommandContext) => {
			if (!remoteTuiAvailable()) {
				ctx.ui.notify("/pi-resume requires PI_GROK_REMOTE_TUI=1", "warning");
				return;
			}

			const switchSession = ctx.switchSession.bind(ctx);
			const sessionManager = ctx.sessionManager;
			await ctx.ui.custom<void>((_tui, _theme, _keybindings, done) => {
				const currentSessionsLoader = (onProgress?: (loaded: number, total: number) => void) =>
					SessionManager.list(sessionManager.getCwd(), sessionManager.getSessionDir(), onProgress);
				const allSessionsLoader = (onProgress?: (loaded: number, total: number) => void) =>
					SessionManager.listAll(sessionManager.getSessionDir(), onProgress);
				const selector = new SessionSelectorComponent(
					currentSessionsLoader,
					allSessionsLoader,
					(sessionPath) => {
						done(undefined);
						void switchSession(sessionPath);
					},
					() => done(undefined),
					() => done(undefined),
					() => {},
					{ showRenameHint: false },
					sessionManager.getSessionFile(),
				);
				return selector;
			});
		},
	});

	pi.registerCommand("pi-reload", {
		description: "Reload Pi resources using Pi's native reload lifecycle",
		handler: async (_args: string, ctx: ExtensionCommandContext) => {
			await ctx.reload();
		},
	});

	const [{ OAuthSelectorComponent }, { LoginDialogComponent }, { ExtensionSelectorComponent }] = await Promise.all([
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

	const selectOAuthOption = async (ctx: ExtensionCommandContext, prompt: OAuthSelectPrompt) => {
		const labels = prompt.options.map((option) => option.label);
		const selectedLabel = await ctx.ui.custom<string | undefined>((_tui, _theme, _keybindings, done) =>
			new ExtensionSelectorComponent(prompt.message, labels, done, () => done(undefined)),
		);
		return prompt.options.find((option) => option.label === selectedLabel)?.id;
	};

	const authenticate = async (ctx: ExtensionCommandContext, provider: AuthSelectorProvider) => {
		if (provider.authType === "api_key") {
			const apiKey = await ctx.ui.custom<string | undefined>((tui, _theme, _keybindings, done) => {
				const dialog = new LoginDialogComponent(tui, provider.id, () => done(undefined), provider.name);
				void dialog.showPrompt("Enter API key:").then(done).catch(() => done(undefined));
				return dialog;
			});
			if (!apiKey?.trim()) return;
			ctx.modelRegistry.authStorage.set(provider.id, { type: "api_key", key: apiKey.trim() });
		} else {
			const completed = await ctx.ui.custom<boolean>((tui, _theme, _keybindings, done) => {
				const dialog = new LoginDialogComponent(tui, provider.id, () => done(false), provider.name);
				void ctx.modelRegistry.authStorage
					.login(provider.id as OAuthProviderId, {
						onAuth: (info) => dialog.showAuth(info.url, info.instructions),
						onDeviceCode: (info) => {
							dialog.showDeviceCode(info);
							dialog.showWaiting("Waiting for authentication...");
						},
						onPrompt: (prompt) => dialog.showPrompt(prompt.message, prompt.placeholder),
						onProgress: (message) => dialog.showProgress(message),
						onSelect: (prompt) => selectOAuthOption(ctx, prompt),
						onManualCodeInput: () => dialog.showManualInput("Paste redirect URL below:"),
						signal: dialog.signal,
					})
					.then(() => done(true))
					.catch((error: unknown) => {
						ctx.ui.notify(`Login failed: ${error instanceof Error ? error.message : String(error)}`, "error");
						done(false);
					});
				return dialog;
			});
			if (!completed) return;
		}

		ctx.modelRegistry.refresh();
		ctx.ui.notify(`Logged in to ${provider.name}`, "success");
	};

	pi.registerCommand("pi-login", {
		description: "[experimental] Log in to a Pi model provider",
		handler: async (args: string, ctx: ExtensionCommandContext) => {
			if (!remoteTuiAvailable()) {
				ctx.ui.notify("/pi-login requires PI_GROK_REMOTE_TUI=1", "warning");
				return;
			}
			const allProviders = loginProviders(ctx);
			const query = args.trim().toLowerCase();
			const matches = query
				? allProviders.filter((provider) => provider.id.toLowerCase() === query || provider.name.toLowerCase() === query)
				: allProviders;
			if (matches.length === 1) {
				await authenticate(ctx, matches[0]!);
				return;
			}
			await ctx.ui.custom<void>((_tui, _theme, _keybindings, done) =>
				new OAuthSelectorComponent(
					"login",
					ctx.modelRegistry.authStorage,
					matches,
					(providerId, authType) => {
						done(undefined);
						const provider = matches.find((item) => item.id === providerId && item.authType === authType);
						if (provider) void authenticate(ctx, provider);
					},
					() => done(undefined),
					(providerId) => ctx.modelRegistry.getProviderAuthStatus(providerId),
					query || undefined,
				),
			);
		},
	});

	pi.registerCommand("pi-logout", {
		description: "[experimental] Remove a stored Pi provider credential",
		handler: async (_args: string, ctx: ExtensionCommandContext) => {
			if (!remoteTuiAvailable()) {
				ctx.ui.notify("/pi-logout requires PI_GROK_REMOTE_TUI=1", "warning");
				return;
			}
			const providers = logoutProviders(ctx);
			if (providers.length === 0) {
				ctx.ui.notify("No credentials saved by /pi-login. Environment variables and models.json are unchanged.", "info");
				return;
			}
			await ctx.ui.custom<void>((_tui, _theme, _keybindings, done) =>
				new OAuthSelectorComponent(
					"logout",
					ctx.modelRegistry.authStorage,
					providers,
					(providerId) => {
						const provider = providers.find((item) => item.id === providerId);
						if (!provider) return;
						ctx.modelRegistry.authStorage.logout(providerId);
						ctx.modelRegistry.refresh();
						done(undefined);
						ctx.ui.notify(`Logged out of ${provider.name}`, "success");
					},
					() => done(undefined),
					(providerId) => ctx.modelRegistry.getProviderAuthStatus(providerId),
				),
			);
		},
	});

	pi.registerCommand("pi-export", unavailableCommand("export", "Pi RPC only exposes HTML export; JSONL export is not public"));
	pi.registerCommand("pi-import", unavailableCommand("import", "Pi RPC has no import-session operation"));
	pi.registerCommand("pi-share", unavailableCommand("share", "Pi RPC has no share-session operation"));
}
