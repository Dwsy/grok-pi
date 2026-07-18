import { expect, mock, test } from "bun:test";

mock.module("@earendil-works/pi-tui", () => ({
  CURSOR_MARKER: "\x1b_pi:c\x07",
  KeybindingsManager: class {},
  TUI_KEYBINDINGS: {},
  matchesKey: () => false,
  setKeybindings: () => {},
}));

const { default: registerRemoteTui } = await import("./index.ts");

test("custom host exposes terminal dimensions to component factories", async () => {
  const previous = process.env.PI_GROK_REMOTE_TUI;
  process.env.PI_GROK_REMOTE_TUI = "1";

  let sessionStart:
    | ((event: unknown, ctx: { ui: { custom: (...args: unknown[]) => unknown; setWidget: () => void } }) => void)
    | undefined;
  const pi = {
    on: (_event: string, handler: typeof sessionStart) => {
      sessionStart = handler;
    },
    registerCommand: () => {},
  };
  const ui = {
    custom: async () => undefined,
    setWidget: () => {},
  };

  try {
    registerRemoteTui(pi as never);
    sessionStart?.({}, { ui });

    const result = await ui.custom((tui: { terminal: { columns: number; rows: number } }, _theme, _kb, done) => {
      expect(tui.terminal).toEqual({ columns: 72, rows: 24 });
      done("ok");
      return { invalidate() {}, render: () => [], handleInput() {} };
    });

    expect(result).toBe("ok");
  } finally {
    if (previous === undefined) delete process.env.PI_GROK_REMOTE_TUI;
    else process.env.PI_GROK_REMOTE_TUI = previous;
  }
});

test("custom host removes Pi hardware cursor markers from projected frames", async () => {
  const previous = process.env.PI_GROK_REMOTE_TUI;
  process.env.PI_GROK_REMOTE_TUI = "1";

  let sessionStart:
    | ((event: unknown, ctx: { ui: { custom: (...args: unknown[]) => unknown; setWidget: (key: string, lines?: string[]) => void } }) => void)
    | undefined;
  const pi = {
    on: (_event: string, handler: typeof sessionStart) => {
      sessionStart = handler;
    },
    registerCommand: () => {},
  };
  let frame: string[] | undefined;
  const ui = {
    custom: async () => undefined,
    setWidget: (_key: string, lines?: string[]) => {
      frame = lines;
    },
  };

  try {
    registerRemoteTui(pi as never);
    sessionStart?.({}, { ui });
    void ui.custom((_tui, _theme, _kb, _done) => ({
      invalidate() {},
      render: () => ["before\x1b_pi:c\x07\x1b[7m \x1b[27mafter"],
      handleInput() {},
    }));
    await new Promise((resolve) => setImmediate(resolve));

    expect(frame).toBeDefined();
    expect(frame?.join("\n")).not.toContain("pi:c");
  } finally {
    if (previous === undefined) delete process.env.PI_GROK_REMOTE_TUI;
    else process.env.PI_GROK_REMOTE_TUI = previous;
  }
});
