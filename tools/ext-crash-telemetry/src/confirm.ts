/**
 * After successful bisection: prompt whether to report.
 * - Y / any key (except N) → report
 * - N / n → do not report
 * Non-TTY → no report (never block CI).
 */

export async function confirmReport(message?: string): Promise<boolean> {
  if (!process.stdin.isTTY || !process.stderr.isTTY) {
    return false;
  }

  const msg =
    message ??
    "Report to telemetry (name + package_dir only)? [Y/n]  (N = no, any other key = yes) ";
  process.stderr.write(msg);

  const key = await readOneKey();
  process.stderr.write("\n");

  if (key === "n" || key === "N") {
    process.stderr.write("Skipped report.\n");
    return false;
  }
  process.stderr.write("Reporting…\n");
  return true;
}

function readOneKey(): Promise<string | null> {
  return new Promise((resolve) => {
    const stdin = process.stdin;
    const wasRaw = stdin.isRaw;
    try {
      stdin.setRawMode?.(true);
    } catch {
      // fall through to line mode
    }
    stdin.resume();
    stdin.setEncoding("utf8");

    const onData = (chunk: string | Buffer) => {
      cleanup();
      const s = String(chunk);
      // Ctrl-C
      if (s === "\u0003") {
        process.stderr.write("^C\n");
        process.exit(130);
      }
      // first printable / letter
      const ch = s[0] ?? null;
      resolve(ch);
    };

    const onEnd = () => {
      cleanup();
      resolve(null);
    };

    const cleanup = () => {
      stdin.off("data", onData);
      stdin.off("end", onEnd);
      try {
        stdin.setRawMode?.(wasRaw ?? false);
      } catch {
        /* ignore */
      }
      stdin.pause();
    };

    stdin.once("data", onData);
    stdin.once("end", onEnd);
  });
}
