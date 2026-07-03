// Regression test for issue #447 / #441:
// pnpm publish strips the executable bit (0644 instead of 0755), causing
// spawnSync to fail with EACCES. The launcher must surface that error clearly
// rather than exiting silently with no message.
//
// Strategy: build a minimal fake install tree in a tmpdir that mirrors what
// `npm install @takazudo/zfb` produces, copy the launcher into it, and run it
// against a non-executable (0644) binary. This avoids NODE_PATH tricks that
// don't work with createRequire(import.meta.url).

import { spawn, spawnSync } from "node:child_process";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
  chmodSync,
  copyFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

const __dirname = dirname(fileURLToPath(import.meta.url));

// Resolve the bin/zfb.mjs launcher path relative to this test file.
// __tests__ is at packages/zfb/src/__tests__, bin is at packages/zfb/bin/
// (two levels up from __tests__: __tests__ → src → zfb/, then bin/).
const launcherPath = resolve(__dirname, "../../bin/zfb.mjs");
// zfb.mjs imports sibling modules (e.g. detect-musl.mjs) that must be
// present alongside it in the fake install tree, exactly like a real
// `npm install` places every file under bin/. Copy the whole directory
// instead of just zfb.mjs so a future sibling module doesn't silently
// break these tests with ERR_MODULE_NOT_FOUND.
const launcherSourceDir = dirname(launcherPath);
function copyBinDir(destDir: string): void {
  mkdirSync(destDir, { recursive: true });
  for (const name of readdirSync(launcherSourceDir)) {
    copyFileSync(join(launcherSourceDir, name), join(destDir, name));
  }
}

// Use the same platform→package mapping as the launcher.
const platformPackages: Record<string, string> = {
  "darwin-arm64": "@takazudo/zfb-darwin-arm64",
  "darwin-x64": "@takazudo/zfb-darwin-x64",
  "linux-arm64": "@takazudo/zfb-linux-arm64-gnu",
  "linux-x64": "@takazudo/zfb-linux-x64-gnu",
  "win32-x64": "@takazudo/zfb-win32-x64-msvc",
};

const platformKey = `${process.platform}-${process.arch}`;
const platformPkg = platformPackages[platformKey];

describe("launcher EACCES surfacing (issue #447)", () => {
  let tmpDir: string;
  let fakeBinPath: string;
  let launcherCopyPath: string;

  beforeEach(() => {
    if (!platformPkg) {
      // Skip setup on unsupported platforms — the test itself will skip below.
      return;
    }

    // Build a fake npm install tree that mirrors what consumers get:
    //   <tmpDir>/
    //     node_modules/
    //       @takazudo/zfb/bin/zfb.mjs       ← copy of the real launcher
    //       <platformPkg>/
    //         package.json
    //         zfb                            ← 0644, intentionally not executable
    tmpDir = mkdtempSync(join(tmpdir(), "zfb-launcher-test-"));

    // Place the launcher copy so createRequire(import.meta.url) roots at
    // <tmpDir>/node_modules/@takazudo/zfb/bin/zfb.mjs, which is the real
    // install layout. createRequire then searches upwards and finds
    // <tmpDir>/node_modules/<platformPkg>/package.json.
    const launcherDir = join(tmpDir, "node_modules", "@takazudo", "zfb", "bin");
    copyBinDir(launcherDir);
    launcherCopyPath = join(launcherDir, "zfb.mjs");

    // Build the fake platform package.
    const pkgDir = join(tmpDir, "node_modules", platformPkg);
    mkdirSync(pkgDir, { recursive: true });

    const binName = process.platform === "win32" ? "zfb.exe" : "zfb";
    fakeBinPath = join(pkgDir, binName);

    // Write a dummy binary (content irrelevant — it won't execute).
    writeFileSync(fakeBinPath, "#!/bin/sh\necho hi\n");

    // Intentionally set 0644 (no execute bit) to reproduce the pnpm-publish bug.
    chmodSync(fakeBinPath, 0o644);

    // Write a minimal package.json so require.resolve can find the package.
    writeFileSync(
      join(pkgDir, "package.json"),
      JSON.stringify({ name: platformPkg, version: "0.0.0" }),
    );
  });

  afterEach(() => {
    if (tmpDir) {
      rmSync(tmpDir, { recursive: true, force: true });
    }
  });

  it("prints an EACCES error message and the binary path when binary has mode 0644", () => {
    if (!platformPkg) {
      // Platform not in the launcher's supported list — cannot construct a
      // fake package layout. Skip rather than fail.
      console.warn(`[skip] unsupported platform: ${platformKey}`);
      return;
    }

    // Spawn the launcher copy so require.resolve roots at the fake install tree.
    const result = spawnSync(process.execPath, [launcherCopyPath], {
      encoding: "utf8",
    });

    // The launcher must exit non-zero.
    expect(result.status).not.toBe(0);

    const stderr = result.stderr ?? "";

    // Must mention the EACCES condition (the words "EACCES" or "not executable").
    expect(stderr).toMatch(/EACCES|not executable/i);

    // Must include the binary path so the user knows exactly which file to fix.
    expect(stderr).toContain(fakeBinPath);
  });
});

// Regression tests for issue #873:
// spawnSync forwards no signals, so SIGTERM to the wrapper orphaned the
// native binary (PPID 1) and left the dev-server port bound. The launcher
// now uses async spawn + signal forwarding + exit/signal propagation.
//
// These tests replace the fake 0644 binary from the shared beforeEach with
// an executable shell script, so they are POSIX-only (skipped on win32 —
// shell scripts cannot be spawned there and signal semantics are emulated).
describe("launcher signal forwarding and exit propagation (issue #873)", () => {
  let tmpDir: string;
  let fakeBinPath: string;
  let launcherCopyPath: string;

  const isPosix = process.platform !== "win32" && !!platformPkg;

  beforeEach(() => {
    if (!isPosix) {
      return;
    }

    tmpDir = mkdtempSync(join(tmpdir(), "zfb-launcher-test-"));

    const launcherDir = join(tmpDir, "node_modules", "@takazudo", "zfb", "bin");
    copyBinDir(launcherDir);
    launcherCopyPath = join(launcherDir, "zfb.mjs");

    const pkgDir = join(tmpDir, "node_modules", platformPkg);
    mkdirSync(pkgDir, { recursive: true });
    fakeBinPath = join(pkgDir, "zfb");
    writeFileSync(
      join(pkgDir, "package.json"),
      JSON.stringify({ name: platformPkg, version: "0.0.0" }),
    );
  });

  afterEach(() => {
    if (tmpDir) {
      rmSync(tmpDir, { recursive: true, force: true });
    }
  });

  /**
   * Write an executable fake platform binary that runs the given JS source
   * under the current Node (absolute-path shebang — no /usr/bin/env or shell
   * dependency, and no intermediate sh process between wrapper and "binary").
   */
  function writeFakeBin(jsSource: string): void {
    writeFileSync(fakeBinPath, `#!${process.execPath}\n${jsSource}`);
    chmodSync(fakeBinPath, 0o755);
  }

  /** Poll until `check` returns true or `timeoutMs` elapses. */
  async function waitFor(check: () => boolean, timeoutMs: number): Promise<boolean> {
    const deadline = Date.now() + timeoutMs;
    while (Date.now() < deadline) {
      if (check()) return true;
      await new Promise((r) => setTimeout(r, 50));
    }
    return check();
  }

  function isProcessAlive(pid: number): boolean {
    try {
      process.kill(pid, 0);
      return true;
    } catch {
      return false;
    }
  }

  it("propagates the child's exit code", () => {
    if (!isPosix) {
      console.warn(`[skip] POSIX-only test on platform: ${platformKey}`);
      return;
    }

    writeFakeBin("process.exit(7);\n");

    const result = spawnSync(process.execPath, [launcherCopyPath], { encoding: "utf8" });

    expect(result.status).toBe(7);
  });

  it("forwards SIGTERM to the native child and dies by the same signal", async () => {
    if (!isPosix) {
      console.warn(`[skip] POSIX-only test on platform: ${platformKey}`);
      return;
    }

    // The fake binary records its PID, then idles — mimicking the
    // long-running `zfb dev` server.
    const pidFile = join(tmpDir, "child.pid");
    writeFakeBin(
      `require("node:fs").writeFileSync(${JSON.stringify(pidFile)}, String(process.pid));\n` +
        `setInterval(() => {}, 1000);\n`,
    );

    const wrapper = spawn(process.execPath, [launcherCopyPath], { stdio: "ignore" });
    const wrapperExit = new Promise<{ code: number | null; signal: string | null }>((res) => {
      wrapper.on("exit", (code, signal) => res({ code, signal }));
    });

    try {
      // Wait for the native child to be up and registered.
      expect(await waitFor(() => existsSync(pidFile), 5000)).toBe(true);
      const childPid = Number(readFileSync(pidFile, "utf8").trim());
      expect(Number.isInteger(childPid) && childPid > 0).toBe(true);
      expect(isProcessAlive(childPid)).toBe(true);

      // Signal ONLY the wrapper — the supervisor scenario from issue #873.
      wrapper.kill("SIGTERM");

      // The native child must be terminated too, not orphaned to PPID 1.
      expect(await waitFor(() => !isProcessAlive(childPid), 5000)).toBe(true);

      // The wrapper must die by the re-raised signal so callers see the
      // real termination cause, not a plain exit code.
      const { signal } = await wrapperExit;
      expect(signal).toBe("SIGTERM");
    } finally {
      // Safety net: never leak the wrapper if an assertion above failed.
      if (wrapper.exitCode === null && wrapper.signalCode === null) {
        wrapper.kill("SIGKILL");
      }
    }
  }, 15000);
});
