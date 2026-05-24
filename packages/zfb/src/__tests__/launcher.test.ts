// Regression test for issue #447 / #441:
// pnpm publish strips the executable bit (0644 instead of 0755), causing
// spawnSync to fail with EACCES. The launcher must surface that error clearly
// rather than exiting silently with no message.
//
// Strategy: build a minimal fake install tree in a tmpdir that mirrors what
// `npm install @takazudo/zfb` produces, copy the launcher into it, and run it
// against a non-executable (0644) binary. This avoids NODE_PATH tricks that
// don't work with createRequire(import.meta.url).

import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync, chmodSync, copyFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

const __dirname = dirname(fileURLToPath(import.meta.url));

// Resolve the bin/zfb.mjs launcher path relative to this test file.
// __tests__ is at packages/zfb/src/__tests__, bin is at packages/zfb/bin/
// (two levels up from __tests__: __tests__ → src → zfb/, then bin/).
const launcherPath = resolve(__dirname, "../../bin/zfb.mjs");

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
    mkdirSync(launcherDir, { recursive: true });
    launcherCopyPath = join(launcherDir, "zfb.mjs");
    copyFileSync(launcherPath, launcherCopyPath);

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
