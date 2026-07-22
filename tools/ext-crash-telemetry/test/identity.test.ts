import { describe, expect, test } from "bun:test";
import { identityFromPath, sanitizeReport } from "../src/identity.ts";

describe("identityFromPath", () => {
  test("scoped npm package", () => {
    const id = identityFromPath(
      "/Users/me/.pi/agent/npm/node_modules/@heyhuynhgiabuu/pi-pretty/index.ts",
    );
    expect(id.ext_name).toBe("pi-pretty");
    expect(id.package_dir).toBe("@heyhuynhgiabuu/pi-pretty");
    expect(id.package_dir).not.toContain("Users");
  });

  test("unscoped npm package", () => {
    const id = identityFromPath(
      "/home/u/.pi/agent/npm/node_modules/pi-tool-display/dist/index.js",
    );
    expect(id.ext_name).toBe("pi-tool-display");
    expect(id.package_dir).toBe("pi-tool-display");
  });

  test("extensions dir", () => {
    const id = identityFromPath(
      "/home/u/.pi/agent/extensions/my-ext/index.ts",
    );
    expect(id.ext_name).toBe("my-ext");
    expect(id.package_dir).toBe("my-ext");
  });

  test("bare file uses parent as package_dir", () => {
    const id = identityFromPath("/tmp/proj/extensions/foo/bar.ts");
    expect(id.package_dir).toBe("foo");
  });
});

describe("sanitizeReport", () => {
  test("accepts whitelist fields", () => {
    const s = sanitizeReport({
      ext_name: "pi-pretty",
      package_dir: "@heyhuynhgiabuu/pi-pretty",
      kind: "crash",
      path: "/secret/home",
    } as never);
    expect(s).toEqual({
      ext_name: "pi-pretty",
      package_dir: "@heyhuynhgiabuu/pi-pretty",
      kind: "crash",
    });
  });

  test("rejects empty", () => {
    expect(sanitizeReport({ kind: "crash" })).toBeNull();
  });

  test("rejects bad kind", () => {
    expect(
      sanitizeReport({
        ext_name: "x",
        package_dir: "x",
        kind: "evil",
      }),
    ).toBeNull();
  });
});
