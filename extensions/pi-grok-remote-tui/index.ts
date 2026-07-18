/**
 * Experimental Remote TUI probe for grok-pi.
 *
 * Enabled only when PI_GROK_REMOTE_TUI=1 (also required by rpc-mode host).
 * Usage in Grok slash: /remote-tui
 *
 * Proves: custom(factory) → in-process Component → frame projection → keys → done.
 */

import type { ExtensionAPI, ExtensionCommandContext } from "@earendil-works/pi-coding-agent";
import { matchesKey, type Component } from "@earendil-works/pi-tui";

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
    const lines = ["Select an item (probe):", ""];
    for (let i = 0; i < ITEMS.length; i++) {
      const item = ITEMS[i]!;
      const mark = i === this.selected ? "→" : " ";
      lines.push(`${mark} ${item.label}  ${item.description}`);
    }
    lines.push("");
    lines.push("Enter confirm · Esc cancel");
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

  pi.registerCommand("remote-tui", {
    description: "[experimental] Remote TUI probe (SelectList over RPC frames)",
    handler: async (_args: string, ctx: ExtensionCommandContext) => {
      const started = Date.now();
      let factoryRan = false;
      const result = await ctx.ui.custom<string | undefined>((_tui, _theme, _kb, done) => {
        factoryRan = true;
        return new RemoteTuiProbeList(done);
      });

      const elapsed = Date.now() - started;
      if (result === undefined && !factoryRan) {
        // RPC stub returns undefined without invoking factory (system Pi without host).
        ctx.ui.notify(
          "Remote TUI host missing: use bundled pi-main (run-local default) and rebuild coding-agent",
          "error",
        );
      } else if (result === undefined && elapsed < 80) {
        ctx.ui.notify(
          "Remote TUI cancelled immediately (host/theme error?). Check PI_BIN=pi-main dist",
          "warning",
        );
      } else if (result === undefined) {
        ctx.ui.notify("Remote TUI cancelled", "info");
      } else {
        ctx.ui.notify(`Remote TUI selected: ${result}`, "info");
      }
    },
  });
}
