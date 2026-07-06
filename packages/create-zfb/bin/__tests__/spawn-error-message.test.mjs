// Regression test for issue #1397:
// create-zfb.mjs's spawnSync() call never inspected `result.error`, so a
// spawn failure (ENOENT/EACCES trying to launch the resolved @takazudo/zfb
// bin/zfb.mjs) silently exited 1 with no message — a broken @takazudo/zfb
// resolution looked like a mystery exit.
//
// Strategy: drive real spawnSync() calls to produce genuine ENOENT/EACCES
// `result.error` objects (rather than hand-constructing fake Error shapes),
// then assert formatSpawnErrorMessage() turns each into an actionable
// message. This mirrors how packages/zfb/src/__tests__/launcher.test.ts
// exercises the sibling zfb.mjs wrapper's EACCES path.

import { spawnSync } from "node:child_process";
import { chmodSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";

import { formatSpawnErrorMessage } from "../spawn-error-message.mjs";

describe("formatSpawnErrorMessage", () => {
  let tmpDir;

  afterEach(() => {
    if (tmpDir) {
      rmSync(tmpDir, { recursive: true, force: true });
      tmpDir = undefined;
    }
  });

  it("formats a real ENOENT spawnSync failure with the missing path and no crash", () => {
    tmpDir = mkdtempSync(join(tmpdir(), "create-zfb-spawn-test-"));
    const missingBin = join(tmpDir, "does-not-exist.mjs");

    // Command itself (not just an arg) must be missing to force
    // result.error — matches create-zfb.mjs's own spawnSync(process.execPath,
    // [zfbBin, ...]) shape being unable to find its own node executable.
    const result = spawnSync(join(tmpDir, "no-such-node-binary"), [missingBin], {
      stdio: "pipe",
    });

    expect(result.error).toBeDefined();
    expect(result.error.code).toBe("ENOENT");

    const message = formatSpawnErrorMessage(result.error, missingBin);
    expect(message).toContain("[create-zfb]");
    expect(message).toContain("failed to spawn @takazudo/zfb");
    expect(message).toContain(missingBin);
  });

  it("formats a real EACCES spawnSync failure with reinstall guidance (mirrors zfb.mjs)", () => {
    if (process.platform === "win32") {
      // Windows has no POSIX execute-bit concept; chmod 0o644 does not
      // reproduce EACCES there. Skip rather than fail.
      console.warn("[skip] POSIX-only test on platform: win32");
      return;
    }

    tmpDir = mkdtempSync(join(tmpdir(), "create-zfb-spawn-test-"));
    const nonExecutable = join(tmpDir, "not-executable.sh");
    writeFileSync(nonExecutable, "#!/bin/sh\necho hi\n");
    chmodSync(nonExecutable, 0o644);

    const result = spawnSync(nonExecutable, [], { stdio: "pipe" });

    expect(result.error).toBeDefined();
    expect(result.error.code).toBe("EACCES");

    const message = formatSpawnErrorMessage(result.error, nonExecutable);
    expect(message).toContain("[create-zfb]");
    expect(message).toContain("not executable");
    expect(message).toContain(nonExecutable);
    expect(message).toContain("npm install --include=optional");
  });
});
