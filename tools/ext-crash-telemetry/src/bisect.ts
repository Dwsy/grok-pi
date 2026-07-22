import { identityFromPath, type ExtIdentity, type ReportKind } from "./identity.ts";

export type ProbeFn = (paths: string[]) => Promise<boolean>;
// true = OK (boots), false = crash/fail

export type BisectResult = {
  kind: ReportKind;
  /** Culprit path when kind=crash; omitted for combo. */
  path?: string;
  identity?: ExtIdentity;
  probes: number;
};

/**
 * VSCode-style extension bisection (mirrors grok-pi self-heal):
 * 1. empty set must pass (caller responsibility optional)
 * 2. find minimal failing prefix via binary search
 * 3. verify single suspect; else scan individuals; else combo
 */
export async function bisectCulprit(
  paths: string[],
  probe: ProbeFn,
): Promise<BisectResult> {
  let probes = 0;
  const run: ProbeFn = async (subset) => {
    probes += 1;
    return probe(subset);
  };

  if (paths.length === 0) {
    return { kind: "unknown", probes };
  }

  // Full set OK → nothing to find
  if (await run(paths)) {
    return { kind: "unknown", probes };
  }

  // Minimal failing prefix
  let lo = 0;
  let hi = paths.length;
  while (lo + 1 < hi) {
    const mid = (lo + hi) >> 1;
    if (await run(paths.slice(0, mid))) {
      lo = mid;
    } else {
      hi = mid;
    }
  }

  const suspect = paths[lo]!;
  if (!(await run([suspect]))) {
    return {
      kind: "crash",
      path: suspect,
      identity: identityFromPath(suspect),
      probes,
    };
  }

  // Combination: try each alone
  for (const p of paths) {
    if (!(await run([p]))) {
      return {
        kind: "crash",
        path: p,
        identity: identityFromPath(p),
        probes,
      };
    }
  }

  return { kind: "combo", probes };
}

/** Build a deterministic mock probe: paths containing any of `bad` fail. */
export function mockProbe(bad: Set<string> | string[]): ProbeFn {
  const set = bad instanceof Set ? bad : new Set(bad);
  return async (paths) => !paths.some((p) => set.has(p));
}
