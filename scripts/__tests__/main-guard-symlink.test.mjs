import { spawnSync } from "node:child_process";
import { mkdtempSync, rmSync, symlinkSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, describe, expect, it } from "vitest";

const scriptsDir = join(dirname(fileURLToPath(import.meta.url)), "..");
const roots = [];

afterEach(() => {
  for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
});

// #4465: the run-main guard used to compare a resolved argv[1] against the
// module's own path, so an invocation through a symlinked directory silently
// exited 0 without running main().
describe("root script main guard", () => {
  it("runs main() when invoked through a symlinked directory", () => {
    const root = mkdtempSync(join(tmpdir(), "zfb-main-guard-"));
    roots.push(root);
    const linked = join(root, "scripts-link");
    symlinkSync(scriptsDir, linked, "dir");

    const result = spawnSync(
      process.execPath,
      [join(linked, "next-changelog-sidebar-position.mjs")],
      { encoding: "utf8" },
    );

    expect(result.status).toBe(1);
    expect(result.stderr).toContain("usage: next-changelog-sidebar-position.mjs");
  });
});
