/** Path → privacy-safe identity (no absolute path leaves this module). */

export type ExtIdentity = {
  /** Short display name (file or package leaf). */
  ext_name: string;
  /** Package / directory label, e.g. `@scope/pkg` or `pi-pretty`. */
  package_dir: string;
};

const MAX_LEN = 128;

function clip(s: string): string {
  const t = s.trim();
  return t.length > MAX_LEN ? t.slice(0, MAX_LEN) : t;
}

/**
 * Derive public identity from an extension path or package-like string.
 * Never returns home/user absolute paths.
 */
export function identityFromPath(input: string): ExtIdentity {
  const raw = input.replace(/\\/g, "/").replace(/\/+$/, "");
  const parts = raw.split("/").filter(Boolean);

  // Prefer node_modules/@scope/name or node_modules/name
  const nm = parts.lastIndexOf("node_modules");
  if (nm >= 0 && nm + 1 < parts.length) {
    const a = parts[nm + 1]!;
    if (a.startsWith("@") && nm + 2 < parts.length) {
      const pkg = `${a}/${parts[nm + 2]!}`;
      return {
        ext_name: clip(parts[nm + 2]!),
        package_dir: clip(pkg),
      };
    }
    return { ext_name: clip(a), package_dir: clip(a) };
  }

  // …/extensions/<dir>/index.ts → package_dir = dir
  const extIdx = parts.lastIndexOf("extensions");
  if (extIdx >= 0 && extIdx + 1 < parts.length) {
    const dir = parts[extIdx + 1]!;
    // skip if next is a file at extensions root
    if (!dir.includes(".")) {
      return { ext_name: clip(dir), package_dir: clip(dir) };
    }
  }

  // basename without extension for files
  const leaf = parts[parts.length - 1] ?? "unknown";
  if (leaf.includes(".") && parts.length >= 2) {
    const parent = parts[parts.length - 2]!;
    if (parent !== "node_modules" && !parent.startsWith(".")) {
      return {
        ext_name: clip(leaf.replace(/\.(ts|js|mjs|cjs)$/i, "")),
        package_dir: clip(parent),
      };
    }
  }

  const name = leaf.replace(/\.(ts|js|mjs|cjs)$/i, "") || "unknown";
  return { ext_name: clip(name), package_dir: clip(name) };
}

/** Sanitize an already-public payload (server-side too). */
export function sanitizeReport(body: {
  ext_name?: unknown;
  package_dir?: unknown;
  kind?: unknown;
}): { ext_name: string; package_dir: string; kind: ReportKind } | null {
  const kind = normalizeKind(body.kind);
  if (!kind) return null;
  const ext_name =
    typeof body.ext_name === "string" ? clip(body.ext_name) : "";
  const package_dir =
    typeof body.package_dir === "string" ? clip(body.package_dir) : "";
  if (!package_dir && !ext_name) return null;
  return {
    ext_name: ext_name || package_dir,
    package_dir: package_dir || ext_name,
    kind,
  };
}

export type ReportKind = "crash" | "combo" | "unknown";

export function normalizeKind(v: unknown): ReportKind | null {
  if (v === "crash" || v === "combo" || v === "unknown") return v;
  if (v === undefined || v === null) return "crash";
  return null;
}

export type ReportPayload = ExtIdentity & {
  kind: ReportKind;
  client?: string;
  grok_pi_ver?: string;
};
