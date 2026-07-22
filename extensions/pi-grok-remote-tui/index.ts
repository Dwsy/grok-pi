/**
 * Experimental Remote TUI — no Pi source patches.
 *
 * Enabled by default from grok-pi (child gets PI_GROK_REMOTE_TUI=1).
 * Disable host process with PI_GROK_REMOTE_TUI=0.
 * 1. On session_start, monkey-patch ctx.ui.custom to run factories in-process.
 * 2. Project frames via existing ctx.ui.setWidget("remote_tui", lines).
 * 3. Keys arrive through a temp keyfile written by the adapter (not Pi RPC).
 *
 * Usage: /remote-tui
 *
 * Demo: multi-select list → Enter applies native surfaces
 * (header/footer widgets, status, title, editor text).
 */

import { randomUUID } from "node:crypto";
import {
  closeSync,
  existsSync,
  openSync,
  readFileSync,
  realpathSync,
  unlinkSync,
  watch,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { pathToFileURL } from "node:url";
import type { ExtensionAPI, ExtensionCommandContext } from "@earendil-works/pi-coding-agent";
import {
  CURSOR_MARKER,
  KeybindingsManager,
  setKeybindings,
  SettingsList,
  TUI_KEYBINDINGS,
  type Component,
  type SettingItem,
  type SettingsListTheme,
} from "@earendil-works/pi-tui";

/** grok-pi RPC needs a custom() host; native Pi TUI already has a real one. */
function shouldInstallRemoteHost(): boolean {
  const flag = process.env.PI_GROK_REMOTE_TUI?.toLowerCase();
  if (flag === "0" || flag === "false" || flag === "off" || flag === "no") {
    return false;
  }
  // grok-pi child always sets PI_GROK=1; native `pi` does not.
  return process.env.PI_GROK === "1" || flag === "1" || flag === "true" || flag === "on" || flag === "yes";
}

function hostUrl(relativePath: string): string {
  const hostDistDir = dirname(realpathSync(process.argv[1]!));
  return new URL(relativePath, pathToFileURL(hostDistDir).href + "/").href;
}

/** Pi interactive components (OAuthSelector/LoginDialog) touch global theme in constructors. */
let themeReady: Promise<void> | null = null;
async function ensurePiTheme(): Promise<void> {
  if (themeReady) return themeReady;
  themeReady = (async () => {
    const mod = (await import(hostUrl("modes/interactive/theme/theme.js"))) as {
      theme?: { name?: string };
      initTheme?: (name?: string, enableWatcher?: boolean) => void;
    };
    try {
      void mod.theme?.name;
    } catch {
      if (typeof mod.initTheme !== "function") {
        throw new Error("Pi theme.initTheme missing");
      }
      mod.initTheme(undefined, false);
      void mod.theme?.name;
    }
  })().catch((error) => {
    themeReady = null;
    throw error;
  });
  return themeReady;
}

const WIDGET_KEY = "remote_tui";
const META_NAME = "pi-grok-remote-tui-active.json";

/** Match Pi interactive TUI: full terminal width, not a fixed probe box. */
function resolveViewport(): { width: number; rows: number } {
  const envWidth = Number(process.env.PI_GROK_REMOTE_TUI_WIDTH);
  const envRows = Number(process.env.PI_GROK_REMOTE_TUI_ROWS);
  const columnsEnv = Number(process.env.COLUMNS);
  const linesEnv = Number(process.env.LINES);
  const stdoutCols = Number(process.stdout?.columns);
  const stdoutRows = Number(process.stdout?.rows);
  // The Pager projects the frame into its padded editor row. Pass the actual
  // terminal width so Pi components wrap before Pager applies its native
  // compact/non-compact outer padding.
  const width = [envWidth, columnsEnv, stdoutCols].find((n) => Number.isFinite(n) && n > 0) ?? 80;
  const rows = [envRows, linesEnv, stdoutRows].find((n) => Number.isFinite(n) && n > 0) ?? 24;
  return { width: Math.max(40, Math.floor(width)), rows: Math.max(8, Math.floor(rows)) };
}

type ComponentLike = Component & { dispose?(): void };
type RemoteTuiDemoUi = {
  setWidget: (key: string, lines: string[] | undefined, options?: { placement?: string }) => void;
  setStatus?: (key: string, text?: string) => void;
  setTitle?: (title: string) => void;
  setEditorText?: (text: string) => void;
};

type ActiveHost = {
  id: string;
  component: ComponentLike;
  closed: boolean;
  width: number;
  keysPath: string;
  watcher: ReturnType<typeof watch> | null;
  keyOffset: number;
  close: (result: unknown) => void;
  pushFrame: () => void;
  handleInput: (data: string) => void;
};

let active: ActiveHost | null = null;
/** Track which uiContext objects already have our custom() host. */
const patchedUIs = new WeakSet<object>();
const HOST_MARK = "__piGrokRemoteTuiHost";

function metaPath(): string {
  return join(tmpdir(), META_NAME);
}

function writeMeta(meta: { id: string; keysPath: string } | null): void {
  const path = metaPath();
  try {
    if (meta === null) {
      if (existsSync(path)) unlinkSync(path);
      return;
    }
    writeFileSync(path, JSON.stringify(meta), "utf8");
  } catch {
    /* ignore */
  }
}

function ensureKeyFile(path: string): void {
  try {
    closeSync(openSync(path, "a"));
  } catch {
    /* ignore */
  }
}

function drainKeys(host: ActiveHost): void {
  if (host.closed) return;
  try {
    if (!existsSync(host.keysPath)) return;
    const buf = readFileSync(host.keysPath, "utf8");
    if (buf.length <= host.keyOffset) return;
    const chunk = buf.slice(host.keyOffset);
    host.keyOffset = buf.length;
    for (const line of chunk.split("\n")) {
      const trimmed = line.trim();
      if (!trimmed) continue;
      let msg: { op?: string; data?: string };
      try {
        msg = JSON.parse(trimmed) as { op?: string; data?: string };
      } catch {
        continue;
      }
      if (msg.op === "cancel") {
        host.close(undefined);
        return;
      }
      if (msg.op === "input" && typeof msg.data === "string") {
        host.handleInput(msg.data);
      }
    }
  } catch {
    /* ignore */
  }
}

function installCustomPatch(ui: RemoteTuiDemoUi & {
  custom: ((...args: unknown[]) => unknown) & { [HOST_MARK]?: boolean };
}): void {
  // Pi may rebind uiContext after session_start (noOp → RPC). Patch every new object.
  if (patchedUIs.has(ui as object) || ui.custom?.[HOST_MARK]) {
    return;
  }
  const original = typeof ui.custom === "function" ? ui.custom.bind(ui) : async () => undefined;

  const hostCustom = async (factory: unknown, _options?: unknown) => {
    if (typeof factory !== "function") {
      return original(factory, _options);
    }

    // Tear down previous session if any
    if (active && !active.closed) {
      active.close(undefined);
    }

    const id = randomUUID();
    const { width, rows } = resolveViewport();
    const keysPath = join(tmpdir(), `pi-grok-remote-tui-keys-${id}.jsonl`);
    ensureKeyFile(keysPath);
    writeMeta({ id, keysPath });

    return new Promise((resolve, reject) => {
      let component: ComponentLike | undefined;
      let closed = false;
      let focused: Component | null = null;
      // Auth select overlays LoginDialog; hide must restore the previous root.
      let previousComponent: ComponentLike | undefined;

      const cleanup = () => {
        try {
          host.watcher?.close();
        } catch {
          /* ignore */
        }
        try {
          if (existsSync(keysPath)) unlinkSync(keysPath);
        } catch {
          /* ignore */
        }
        writeMeta(null);
        // Clear only the interactive frame. Applied demo surfaces stay so
        // header/footer/status can still be inspected after Esc.
        ui.setWidget(WIDGET_KEY, undefined);
        if (active?.id === id) active = null;
        try {
          component?.dispose?.();
        } catch {
          /* ignore */
        }
      };

      const close = (result: unknown) => {
        if (closed) return;
        closed = true;
        host.closed = true;
        cleanup();
        resolve(result);
      };

      const pushFrame = () => {
        if (closed || !component) return;
        try {
          // Pi components emit this APC sequence only for their in-process
          // terminal renderer to position a hardware cursor. Pager renders the
          // projected frame itself, so forwarding it leaks its `pi:c` payload.
          const lines = component
            .render(width)
            .map((line) => String(line).replaceAll(CURSOR_MARKER, ""));
          ui.setWidget(WIDGET_KEY, lines, { placement: "aboveEditor" });
        } catch (error) {
          if (closed) return;
          closed = true;
          host.closed = true;
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
            host.closed = true;
            cleanup();
            reject(error instanceof Error ? error : new Error(String(error)));
            return;
          }
        }
        pushFrame();
      };

      const tuiStub = {
        terminal: { columns: width, rows },
        requestRender: () => {
          process.nextTick(() => {
            if (!closed) pushFrame();
          });
        },
        setFocus: (next: Component | null) => {
          focused = next;
        },
        showOverlay: (overlay: Component) => {
          if (component && component !== overlay) {
            previousComponent = component;
          }
          component = overlay as ComponentLike;
          focused = overlay;
          pushFrame();
          return {
            hide: () => {
              if (closed) return;
              if (previousComponent) {
                component = previousComponent;
                focused = previousComponent;
                previousComponent = undefined;
                pushFrame();
                return;
              }
              // No stacked root (e.g. standalone selector) — keep current frame.
              focused = component;
              pushFrame();
            },
            show: () => pushFrame(),
            setVisible: (visible: boolean) => {
              if (!visible) {
                tuiStub.hideOverlay();
              } else {
                pushFrame();
              }
            },
          };
        },
        hideOverlay: () => {
          if (closed) return;
          if (previousComponent) {
            component = previousComponent;
            focused = previousComponent;
            previousComponent = undefined;
            pushFrame();
          }
        },
        addChild: () => {},
        removeChild: () => {},
      };

      const host: ActiveHost = {
        id,
        component: undefined as unknown as ComponentLike,
        closed: false,
        width,
        keysPath,
        watcher: null,
        keyOffset: 0,
        close,
        pushFrame,
        handleInput,
      };
      active = host;

      // Minimal theme: color helpers return ANSI so frames can render in Grok.
      const themeStub = new Proxy(
        {},
        {
          get: (_t, prop) => {
            if (prop === "fg") {
              return (color: string, text: string) => {
                const codes: Record<string, string> = {
                  accent: "36",
                  success: "32",
                  error: "31",
                  warning: "33",
                  dim: "2",
                  muted: "2",
                  border: "90",
                };
                const code = codes[color] ?? "0";
                return `\x1b[${code}m${text}\x1b[0m`;
              };
            }
            if (prop === "bold") return (text: string) => `\x1b[1m${text}\x1b[0m`;
            if (prop === "name") return "remote-tui-stub";
            return () => "";
          },
        },
      );

      // Real keybindings manager — many Pi components call keybindings.matches(...).
      // Empty {} caused: "this.keybindings.matches is not a function".
      const keybindings = new KeybindingsManager(TUI_KEYBINDINGS, {});
      setKeybindings(keybindings);

      try {
        host.watcher = watch(keysPath, () => drainKeys(host));
      } catch {
        // poll fallback
        const timer = setInterval(() => {
          if (host.closed) {
            clearInterval(timer);
            return;
          }
          drainKeys(host);
        }, 50);
      }

      // Prefer Pi theme when available (OAuthSelector/LoginDialog touch it).
      // Fall back to themeStub for unit tests / non-Pi argv hosts.
      void ensurePiTheme()
        .catch(() => undefined)
        .then(() =>
          (factory as (tui: unknown, theme: unknown, kb: unknown, done: (r: unknown) => void) => unknown)(
            tuiStub,
            themeStub,
            keybindings,
            close,
          ),
        )
        .then((created) => {
          if (closed) {
            try {
              (created as ComponentLike)?.dispose?.();
            } catch {
              /* ignore */
            }
            return;
          }
          component = created as ComponentLike;
          host.component = component;
          focused = component;
          pushFrame();
          drainKeys(host);
        })
        .catch((error) => {
          if (closed) return;
          closed = true;
          host.closed = true;
          cleanup();
          reject(error instanceof Error ? error : new Error(String(error)));
        });
    });
  };

  (hostCustom as typeof hostCustom & { [HOST_MARK]?: boolean })[HOST_MARK] = true;
  ui.custom = hostCustom as typeof ui.custom;
  patchedUIs.add(ui as object);
}

/** Other host-injected extensions (auth login/logout) re-bind after RPC ui swaps. */
function ensureRemoteTuiHost(ui: Parameters<typeof installCustomPatch>[0]): void {
  if (!shouldInstallRemoteHost()) return;
  installCustomPatch(ui);
}

(globalThis as typeof globalThis & {
  __piGrokEnsureRemoteTuiHost?: typeof ensureRemoteTuiHost;
}).__piGrokEnsureRemoteTuiHost = ensureRemoteTuiHost;

export const DEMO_ITEMS = [
  { key: "header", label: "Header widget", description: "aboveEditor native surface" },
  { key: "footer", label: "Footer widget", description: "belowEditor native surface" },
  { key: "status", label: "Status bar", description: "setStatus fire-and-forget" },
  { key: "title", label: "Window title", description: "setTitle fire-and-forget" },
  { key: "editor", label: "Prompt editor", description: "setEditorText fire-and-forget" },
] as const;

export type DemoKey = (typeof DEMO_ITEMS)[number]["key"];

export function applyDemoCapabilities(ui: RemoteTuiDemoUi, keys: DemoKey[]): void {
  const selected = new Set(keys);
  const labels = DEMO_ITEMS.filter((item) => selected.has(item.key)).map((item) => item.label);
  const summary = labels.length > 0 ? labels.join(", ") : "none";

  // Align with Pi setWidget semantics: plain multi-line frames above/below
  // the editor. No synthetic "Esc closes" chrome — Esc is host cancellation.
  ui.setWidget(
    "remote_tui_demo_header",
    selected.has("header")
      ? [
          "\x1b[1mRemote TUI demo header\x1b[0m",
          `\x1b[2m${summary}\x1b[0m`,
        ]
      : undefined,
    { placement: "aboveEditor" },
  );
  ui.setWidget(
    "remote_tui_demo_footer",
    selected.has("footer")
      ? [
          `\x1b[2mFooter · ${labels.length} selected: ${summary}\x1b[0m`,
        ]
      : undefined,
    { placement: "belowEditor" },
  );
  if (selected.has("status")) {
    ui.setStatus?.("remote-tui-demo", `Remote TUI demo: ${summary}`);
  } else {
    ui.setStatus?.("remote-tui-demo");
  }
  if (selected.has("title")) {
    ui.setTitle?.("Remote TUI capability lab");
  }
  if (selected.has("editor")) {
    ui.setEditorText?.("Remote TUI demo applied — type here or press Esc to close.");
  }
}

function demoSettingsTheme(theme: {
  fg: (color: string, text: string) => string;
  bold?: (text: string) => string;
}): SettingsListTheme {
  return {
    label: (text, selected) => (selected ? theme.fg("accent", text) : text),
    value: (text, selected) =>
      selected ? theme.fg("accent", text) : theme.fg("dim", text),
    description: (text) => theme.fg("dim", text),
    cursor: theme.fg("accent", "→ "),
    hint: (text) => theme.fg("dim", text),
  };
}

/** Native Pi SettingsList multi-toggle — works under real TUI and remote host. */
export function createDemoSelector(
  tui: { requestRender: () => void },
  theme: {
    fg: (color: string, text: string) => string;
    bold?: (text: string) => string;
  },
  done: (result: string | undefined) => void,
  onChange: (keys: DemoKey[]) => void,
): Component {
  const enabled = new Set<DemoKey>();
  const items: SettingItem[] = DEMO_ITEMS.map((item) => ({
    id: item.key,
    label: item.label,
    description: item.description,
    currentValue: "off",
    values: ["on", "off"],
  }));

  const selectedKeys = (): DemoKey[] =>
    DEMO_ITEMS.map((item) => item.key).filter((key) => enabled.has(key));

  const list = new SettingsList(
    items,
    DEMO_ITEMS.length + 1,
    demoSettingsTheme(theme),
    (id, newValue) => {
      if (newValue === "on") enabled.add(id as DemoKey);
      else enabled.delete(id as DemoKey);
      onChange(selectedKeys());
    },
    () => {
      const keys = selectedKeys();
      done(keys.length > 0 ? keys.join(",") : undefined);
    },
  );

  const bold = theme.bold ?? ((text: string) => text);
  return {
    invalidate() {
      list.invalidate();
    },
    render(width: number) {
      return [
        theme.fg("accent", bold("Remote TUI capability lab")),
        theme.fg("dim", "Enter/Space toggle · Esc close"),
        "",
        ...list.render(width),
      ];
    },
    handleInput(data: string) {
      list.handleInput(data);
      tui.requestRender();
    },
  };
}

export default function (pi: ExtensionAPI) {
  pi.on("session_start", (_event, ctx) => {
    ensureRemoteTuiHost(ctx.ui as Parameters<typeof installCustomPatch>[0]);
  });

  pi.registerCommand("remote-tui", {
    description: "[experimental] Remote TUI capability lab with selectable widgets",
    handler: async (_args: string, ctx: ExtensionCommandContext) => {
      // grok-pi RPC only: re-bind host after uiContext swaps. Native TUI: leave custom alone.
      ensureRemoteTuiHost(ctx.ui as Parameters<typeof installCustomPatch>[0]);

      const started = Date.now();
      let factoryRan = false;
      const openDemo = () =>
        ctx.ui.custom<string | undefined>((tui, theme, _kb, done) => {
          factoryRan = true;
          return createDemoSelector(
            tui as { requestRender: () => void },
            theme as {
              fg: (color: string, text: string) => string;
              bold?: (text: string) => string;
            },
            done,
            (keys) => applyDemoCapabilities(ctx.ui as RemoteTuiDemoUi, keys),
          );
        });

      const result = await openDemo();

      const elapsed = Date.now() - started;
      if (result === undefined && !factoryRan) {
        if (shouldInstallRemoteHost()) {
          installCustomPatch(ctx.ui as Parameters<typeof installCustomPatch>[0]);
          const retry = await openDemo();
          if (retry !== undefined || factoryRan) {
            if (retry === undefined) ctx.ui.notify("Remote TUI demo closed", "info");
            else ctx.ui.notify(`Remote TUI demo applied: ${retry}`, "info");
            return;
          }
          ctx.ui.notify(
            "Remote TUI host patch failed: custom() stub still active (rebuild grok-pi)",
            "error",
          );
          return;
        }
        ctx.ui.notify(
          "custom() unavailable (RPC without remote host). Run under grok-pi or native Pi TUI.",
          "error",
        );
      } else if (result === undefined && elapsed < 80) {
        ctx.ui.notify("Remote TUI cancelled immediately", "warning");
      } else if (result === undefined) {
        ctx.ui.notify("Remote TUI demo closed", "info");
      } else {
        ctx.ui.notify(`Remote TUI demo applied: ${result}`, "info");
      }
    },
  });
}
