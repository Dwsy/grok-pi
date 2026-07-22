#!/usr/bin/env bun
/**
 * ext-bisect — local extension crash bisection + optional CF report.
 *
 *   bun run src/cli.ts probe --bad a.ts --paths a.ts b.ts c.ts
 *   bun run src/cli.ts probe --paths-file list.txt --mock-bad cul.ts
 *   bun run src/cli.ts report --json '{"ext_name":"x","package_dir":"x","kind":"crash"}'
 *   bun run src/cli.ts top
 */
import { bisectCulprit, mockProbe } from "./bisect.ts";
import { confirmReport } from "./confirm.ts";
import { identityFromPath, type ReportPayload } from "./identity.ts";
import { fetchStats, postReport } from "./report.ts";

const DEFAULT_REPORT_URL =
  "https://ext-crash-telemetry.dwsycode.workers.dev";

async function resolveReportToken(): Promise<string | undefined> {
  if (process.env.REPORT_TOKEN?.trim()) return process.env.REPORT_TOKEN.trim();
  // tools/ext-crash-telemetry/.env (gitignored)
  try {
    const envPath = new URL("../.env", import.meta.url);
    const text = await Bun.file(envPath).text();
    const m = text.match(/^REPORT_TOKEN=(.+)$/m);
    if (m?.[1]) return m[1].trim();
  } catch {
    /* ignore */
  }
  // ~/.grok-pi/ext-telemetry.token
  try {
    const home = process.env.HOME || process.env.USERPROFILE;
    if (home) {
      const p = `${home}/.grok-pi/ext-telemetry.token`;
      const t = (await Bun.file(p).text()).trim();
      if (t) return t;
    }
  } catch {
    /* ignore */
  }
  return undefined;
}

function usage(): never {
  console.error(`ext-bisect — extension crash bisection telemetry CLI

Usage:
  ext-bisect probe  --paths <p1> [p2...] [--mock-bad <p>]...
  ext-bisect probe  --paths-file <file> [--mock-bad <p>]...
  ext-bisect report --json <payload> | --stdin
  ext-bisect top

Env:
  REPORT_URL     Worker base URL (default: production worker)
  REPORT_TOKEN   Optional Bearer token for write/admin

After a successful probe, prompts: Y / any key = report, N = skip.
`);
  process.exit(2);
}

function argList(argv: string[], flag: string): string[] {
  const out: string[] = [];
  for (let i = 0; i < argv.length; i++) {
    if (argv[i] === flag) {
      i++;
      while (i < argv.length && !argv[i]!.startsWith("--")) {
        out.push(argv[i]!);
        i++;
      }
      i--;
    }
  }
  return out;
}

function argOne(argv: string[], flag: string): string | undefined {
  const i = argv.indexOf(flag);
  if (i >= 0 && i + 1 < argv.length) return argv[i + 1];
  return undefined;
}

async function cmdProbe(argv: string[]) {
  let paths = argList(argv, "--paths");
  const file = argOne(argv, "--paths-file");
  if (file) {
    const text = await Bun.file(file).text();
    paths = [
      ...paths,
      ...text
        .split(/\r?\n/)
        .map((l) => l.trim())
        .filter((l) => l && !l.startsWith("#")),
    ];
  }
  if (paths.length === 0) {
    console.error("probe: need --paths or --paths-file");
    process.exit(1);
  }

  const mockBad = argList(argv, "--mock-bad");
  if (mockBad.length === 0) {
    console.error(
      "probe: real Pi spawn not wired in S1; pass --mock-bad <path> for dry-run bisection",
    );
    process.exit(1);
  }

  const noReport = argv.includes("--no-report");
  const forceReport = argv.includes("--report");

  const result = await bisectCulprit(paths, mockProbe(mockBad));
  const out = {
    kind: result.kind,
    probes: result.probes,
    ...(result.identity ?? {}),
    // never echo absolute path in final report shape; keep under _debug only
    _debug_path: result.path,
  };
  console.log(JSON.stringify(out, null, 2));

  // Auto-report gate: only after a real finding (crash / combo).
  if (result.kind !== "crash" && result.kind !== "combo") return;
  if (noReport) return;

  const endpoint =
    process.env.REPORT_URL?.trim() || DEFAULT_REPORT_URL;

  let should = forceReport;
  if (!should) {
    should = await confirmReport(
      `Report ${result.kind} to telemetry (ext_name + package_dir only)? [Y/n]  (N = no, any other key = yes) `,
    );
  }
  if (!should) return;

  const identity =
    result.identity ??
    (result.kind === "combo"
      ? { ext_name: "combo", package_dir: "combo" }
      : identityFromPath(result.path ?? "unknown"));

  const token = await resolveReportToken();
  if (!token) {
    console.error(
      "REPORT_TOKEN missing (env, .env, or ~/.grok-pi/ext-telemetry.token); skip report",
    );
    return;
  }
  const res = await postReport(
    endpoint,
    {
      ext_name: identity.ext_name,
      package_dir: identity.package_dir,
      kind: result.kind,
      client: "ext-bisect-cli",
    },
    { token },
  );
  console.error(JSON.stringify(res));
  if (!res.ok) process.exitCode = 1;
}

async function cmdReport(argv: string[]) {
  const endpoint = process.env.REPORT_URL;
  if (!endpoint) {
    console.error("REPORT_URL required");
    process.exit(1);
  }

  let raw: string;
  if (argv.includes("--stdin")) {
    raw = await new Response(Bun.stdin.stream()).text();
  } else {
    const j = argOne(argv, "--json");
    if (!j) {
      console.error("report: --json or --stdin");
      process.exit(1);
    }
    raw = j;
  }

  const parsed = JSON.parse(raw) as ReportPayload;
  // Strip accidental path fields
  const payload: ReportPayload = {
    ext_name: parsed.ext_name ?? identityFromPath(String((parsed as { path?: string }).path ?? "")).ext_name,
    package_dir:
      parsed.package_dir ??
      identityFromPath(String((parsed as { path?: string }).path ?? "")).package_dir,
    kind: parsed.kind ?? "crash",
    client: parsed.client ?? "ext-bisect-cli",
    grok_pi_ver: parsed.grok_pi_ver,
  };

  const token = await resolveReportToken();
  if (!token) {
    console.error("REPORT_TOKEN required (env / .env / ~/.grok-pi/ext-telemetry.token)");
    process.exit(1);
  }
  const res = await postReport(endpoint, payload, { token });
  console.log(JSON.stringify(res, null, 2));
  if (!res.ok) process.exit(1);
}

async function cmdTop() {
  const endpoint = process.env.REPORT_URL;
  if (!endpoint) {
    console.error("REPORT_URL required");
    process.exit(1);
  }
  const stats = await fetchStats(endpoint, {
    token: process.env.REPORT_TOKEN,
  });
  console.log(JSON.stringify(stats, null, 2));
}

async function main() {
  const argv = process.argv.slice(2);
  const cmd = argv[0];
  if (!cmd || cmd === "-h" || cmd === "--help") usage();
  if (cmd === "probe") return cmdProbe(argv.slice(1));
  if (cmd === "report") return cmdReport(argv.slice(1));
  if (cmd === "top") return cmdTop();
  usage();
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
