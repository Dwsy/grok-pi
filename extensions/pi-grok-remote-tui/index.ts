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
 */

import { randomUUID } from "node:crypto";
import {
  closeSync,
  existsSync,
  openSync,
  readFileSync,
  unlinkSync,
  watch,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { ExtensionAPI, ExtensionCommandContext } from "@earendil-works/pi-coding-agent";
import {
  CURSOR_MARKER,
  KeybindingsManager,
  matchesKey,
  setKeybindings,
  TUI_KEYBINDINGS,
  type Component,
} from "@earendil-works/pi-tui";

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
  // Prefer explicit host size from grok-pi (COLUMNS / PI_GROK_REMOTE_TUI_WIDTH).
  const width = [envWidth, columnsEnv, stdoutCols].find((n) => Number.isFinite(n) && n > 0) ?? 80;
  const rows = [envRows, linesEnv, stdoutRows].find((n) => Number.isFinite(n) && n > 0) ?? 24;
  return { width: Math.max(40, Math.floor(width)), rows: Math.max(8, Math.floor(rows)) };
}

type ComponentLike = Component & { dispose?(): void };

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

function installCustomPatch(ui: {
  custom: ((...args: unknown[]) => unknown) & { [HOST_MARK]?: boolean };
  setWidget: (key: string, lines: string[] | undefined, options?: { placement?: string }) => void;
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
          const frame = [
            ...lines,
            "\x1b[2m↑/↓ · Enter · Esc\x1b[0m",
          ];
          ui.setWidget(WIDGET_KEY, frame, { placement: "aboveEditor" });
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
          component = overlay as ComponentLike;
          focused = overlay;
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

      Promise.resolve(
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
          reject(error);
        });
    });
  };

  (hostCustom as typeof hostCustom & { [HOST_MARK]?: boolean })[HOST_MARK] = true;
  ui.custom = hostCustom as typeof ui.custom;
  patchedUIs.add(ui as object);
}

const ITEMS = [
  { value: "alpha", label: "Alpha", description: "first choice" },
  { value: "beta", label: "Beta", description: "second choice" },
  { value: "gamma", label: "Gamma", description: "third choice" },
  { value: "delta", label: "Delta", description: "fourth choice" },
] as const;

class RemoteTuiProbeList implements Component {
  private selected = 0;
  private done: (result: string | undefined) => void;

  constructor(done: (result: string | undefined) => void) {
    this.done = done;
  }

  invalidate(): void {}

  render(_width: number): string[] {
    const lines = ["\x1b[1mSelect an item (probe):\x1b[0m", ""];
    for (let i = 0; i < ITEMS.length; i++) {
      const item = ITEMS[i]!;
      if (i === this.selected) {
        lines.push(`\x1b[36m→ ${item.label}\x1b[0m  \x1b[2m${item.description}\x1b[0m`);
      } else {
        lines.push(`  ${item.label}  \x1b[2m${item.description}\x1b[0m`);
      }
    }
    lines.push("");
    lines.push("\x1b[2mEnter confirm · Esc cancel\x1b[0m");
    return lines;
  }

  handleInput(data: string): void {
    if (matchesKey(data, "up") || data === "k") {
      this.selected = this.selected === 0 ? ITEMS.length - 1 : this.selected - 1;
      return;
    }
    if (matchesKey(data, "down") || data === "j") {
      this.selected = this.selected === ITEMS.length - 1 ? 0 : this.selected + 1;
      return;
    }
    if (matchesKey(data, "enter") || matchesKey(data, "return")) {
      this.done(ITEMS[this.selected]!.value);
      return;
    }
    if (matchesKey(data, "escape")) {
      this.done(undefined);
    }
  }
}

export default function (pi: ExtensionAPI) {
  if (process.env.PI_GROK_REMOTE_TUI !== "1") {
    return;
  }

  pi.on("session_start", (_event, ctx) => {
    installCustomPatch(ctx.ui as Parameters<typeof installCustomPatch>[0]);
  });

  // Also patch immediately if UI already bound (command registration time).
  // session_start is the reliable hook; command handler double-checks.

  pi.registerCommand("remote-tui", {
    description: "[experimental] Remote TUI probe (extension host, no Pi source patch)",
    handler: async (_args: string, ctx: ExtensionCommandContext) => {
      // Always re-check: RPC rebinds uiContext after extension load / session_start.
      installCustomPatch(ctx.ui as Parameters<typeof installCustomPatch>[0]);

      const started = Date.now();
      let factoryRan = false;
      const result = await ctx.ui.custom<string | undefined>((_tui, _theme, _kb, done) => {
        factoryRan = true;
        return new RemoteTuiProbeList(done);
      });

      const elapsed = Date.now() - started;
      if (result === undefined && !factoryRan) {
        // One more attempt in case ui reference changed mid-handler.
        installCustomPatch(ctx.ui as Parameters<typeof installCustomPatch>[0]);
        const retry = await ctx.ui.custom<string | undefined>((_tui, _theme, _kb, done) => {
          factoryRan = true;
          return new RemoteTuiProbeList(done);
        });
        if (retry !== undefined || factoryRan) {
          if (retry === undefined) ctx.ui.notify("Remote TUI cancelled", "info");
          else ctx.ui.notify(`Remote TUI selected: ${retry}`, "info");
          return;
        }
        ctx.ui.notify(
          "Remote TUI host patch failed: custom() stub still active (rebuild grok-pi)",
          "error",
        );
      } else if (result === undefined && elapsed < 80) {
        ctx.ui.notify("Remote TUI cancelled immediately", "warning");
      } else if (result === undefined) {
        ctx.ui.notify("Remote TUI cancelled", "info");
      } else {
        ctx.ui.notify(`Remote TUI selected: ${result}`, "info");
      }
    },
  });
}
