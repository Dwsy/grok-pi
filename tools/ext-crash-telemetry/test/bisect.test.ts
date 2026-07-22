import { describe, expect, test } from "bun:test";
import { bisectCulprit, mockProbe } from "../src/bisect.ts";

describe("bisectCulprit", () => {
  test("finds single culprit mid-list", async () => {
    const paths = ["a", "b", "c", "d", "e"];
    const r = await bisectCulprit(paths, mockProbe(["c"]));
    expect(r.kind).toBe("crash");
    expect(r.path).toBe("c");
    expect(r.identity?.ext_name).toBe("c");
    expect(r.probes).toBeGreaterThan(0);
  });

  test("finds first item", async () => {
    const r = await bisectCulprit(["bad", "ok"], mockProbe(["bad"]));
    expect(r.kind).toBe("crash");
    expect(r.path).toBe("bad");
  });

  test("finds last item", async () => {
    const r = await bisectCulprit(["ok1", "ok2", "bad"], mockProbe(["bad"]));
    expect(r.kind).toBe("crash");
    expect(r.path).toBe("bad");
  });

  test("combo when no single fails alone", async () => {
    // Fail only when both present — mockProbe uses "any bad in set" so we need custom probe
    const probe = async (paths: string[]) => {
      const hasA = paths.includes("a");
      const hasB = paths.includes("b");
      // crash only if both a and b loaded together
      if (hasA && hasB) return false;
      return true;
    };
    const r = await bisectCulprit(["a", "b", "c"], probe);
    expect(r.kind).toBe("combo");
    expect(r.path).toBeUndefined();
  });

  test("unknown when full set ok", async () => {
    const r = await bisectCulprit(["a", "b"], mockProbe([]));
    expect(r.kind).toBe("unknown");
  });

  test("empty paths", async () => {
    const r = await bisectCulprit([], mockProbe(["x"]));
    expect(r.kind).toBe("unknown");
  });
});
