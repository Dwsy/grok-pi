const BUILTIN_TOOLS = ["read", "bash", "edit", "write", "grep", "find", "ls"] as const;

/**
 * Applies the host's persisted grok-pi built-in-tool preference at session
 * startup. It deliberately leaves extension and custom tools untouched.
 */
export default function (pi: {
  on(event: "session_start", handler: () => void): void;
  getActiveTools(): string[];
  setActiveTools(toolNames: string[]): void;
}) {
  const configured = process.env.PI_GROK_BUILTIN_TOOLS;
  if (!configured) return;

  const selected = configured
    .split(",")
    .filter((name): name is (typeof BUILTIN_TOOLS)[number] =>
      (BUILTIN_TOOLS as readonly string[]).includes(name),
    );

  pi.on("session_start", () => {
    const builtin = new Set<string>(BUILTIN_TOOLS);
    const activeNonBuiltin = pi.getActiveTools().filter((name) => !builtin.has(name));
    pi.setActiveTools([...activeNonBuiltin, ...selected]);
  });
}
