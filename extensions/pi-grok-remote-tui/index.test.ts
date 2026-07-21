import { expect, mock, test } from "bun:test";

mock.module("@earendil-works/pi-tui", () => ({
  CURSOR_MARKER: "\x1b_pi:c\x07",
  KeybindingsManager: class {
    matches() {
      return false;
    }
  },
  TUI_KEYBINDINGS: {},
  matchesKey: (data: string, key: string) => {
    if (key === "up") return data === "\x1b[A";
    if (key === "down") return data === "\x1b[B";
    if (key === "enter" || key === "return") return data === "\r";
    if (key === "escape") return data === "\x1b";
    if (key === "space") return data === " ";
    return false;
  },
  setKeybindings: () => {},
}));

const {
  default: registerRemoteTui,
  RemoteTuiDemoList,
  applyDemoCapabilities,
} = await import("./index.ts");

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
      expect(tui.terminal.columns).toBeGreaterThan(0);
      expect(tui.terminal.rows).toBeGreaterThan(0);
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
    await new Promise((resolve) => setImmediate(resolve));

    expect(frame).toBeDefined();
    expect(frame?.join("\n")).not.toContain("pi:c");
  } finally {
    if (previous === undefined) delete process.env.PI_GROK_REMOTE_TUI;
    else process.env.PI_GROK_REMOTE_TUI = previous;
  }
});

test("demo list toggles checkboxes and applies selected surfaces", () => {
  const applied: string[][] = [];
  let closed: string | undefined;
  const demo = new RemoteTuiDemoList(
    (result) => {
      closed = result;
    },
    (keys) => {
      applied.push([...keys]);
    },
  );

  // Space on first item (header)
  demo.handleInput(" ");
  // Move down and check footer
  demo.handleInput("\x1b[B");
  demo.handleInput(" ");
  // Apply without closing
  demo.handleInput("\r");
  expect(applied).toEqual([["header", "footer"]]);

  const rendered = demo.render(80).join("\n");
  expect(rendered).toContain("Remote TUI capability lab");
  expect(rendered).toContain("☑");
  expect(rendered).toContain("Applied. Inspect the native header/footer/status surfaces.");

  // Esc closes with selected keys
  demo.handleInput("\x1b");
  expect(closed).toBe("header,footer");
});

test("applyDemoCapabilities projects header/footer/status/title/editor", () => {
  const widgets = new Map<string, { lines?: string[]; placement?: string }>();
  let status: { key?: string; text?: string } = {};
  let title: string | undefined;
  let editorText: string | undefined;

  applyDemoCapabilities(
    {
      setWidget: (key, lines, options) => {
        widgets.set(key, { lines, placement: options?.placement });
      },
      setStatus: (key, text) => {
        status = { key, text };
      },
      setTitle: (value) => {
        title = value;
      },
      setEditorText: (value) => {
        editorText = value;
      },
    },
    ["header", "footer", "status", "title", "editor"],
  );

  expect(widgets.get("remote_tui_demo_header")?.placement).toBe("aboveEditor");
  expect(widgets.get("remote_tui_demo_header")?.lines?.join("\n")).toContain("Remote TUI demo header");
  expect(widgets.get("remote_tui_demo_footer")?.placement).toBe("belowEditor");
  expect(widgets.get("remote_tui_demo_footer")?.lines?.join("\n")).toContain("Footer · 5 selected");
  expect(widgets.get("remote_tui_demo_footer")?.lines?.join("\n")).not.toContain("Esc");
  expect(status).toEqual({
    key: "remote-tui-demo",
    text: "Remote TUI demo: Header widget, Footer widget, Status bar, Window title, Prompt editor",
  });
  expect(title).toBe("Remote TUI capability lab");
  expect(editorText).toContain("Remote TUI demo applied");
});

test("showOverlay restores previous root component on hide", async () => {
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

    let tuiRef: {
      showOverlay: (component: {
        render: () => string[];
        handleInput?: () => void;
        invalidate?: () => void;
      }) => { hide: () => void };
    } | undefined;

    void ui.custom((tui, _theme, _kb, _done) => {
      tuiRef = tui as typeof tuiRef;
      return {
        invalidate() {},
        render: () => ["root-frame"],
        handleInput() {},
      };
    });

    await new Promise((resolve) => setImmediate(resolve));
    await new Promise((resolve) => setImmediate(resolve));
    expect(frame?.join("\n")).toContain("root-frame");

    const handle = tuiRef!.showOverlay({
      invalidate() {},
      render: () => ["overlay-frame"],
      handleInput() {},
    });
    await new Promise((resolve) => setImmediate(resolve));
    expect(frame?.join("\n")).toContain("overlay-frame");

    handle.hide();
    await new Promise((resolve) => setImmediate(resolve));
    expect(frame?.join("\n")).toContain("root-frame");
  } finally {
    if (previous === undefined) delete process.env.PI_GROK_REMOTE_TUI;
    else process.env.PI_GROK_REMOTE_TUI = previous;
  }
});

