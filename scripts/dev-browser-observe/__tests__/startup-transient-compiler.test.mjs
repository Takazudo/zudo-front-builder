import assert from "node:assert/strict";
import { execFile, execFileSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";
import { test } from "vitest";

const run = promisify(execFile);
const fixture = fileURLToPath(new URL("./startup-transient-compiler.py", import.meta.url));
const repo = fileURLToPath(new URL("../../..", import.meta.url));
const onLinux = process.platform === "linux";

test.runIf(onLinux)(
  "the startup scout accepts only a status-0 transient compiler worker as first seen terminal",
  async () => {
    const { stdout } = await run("python3", ["-I", "-B", fixture], { timeout: 60_000 });
    assert.match(stdout.trim(), /^PASS /);
  },
);

test.runIf(onLinux)(
  "the r4 shape refuses on f8ab37bfb and is a completed transient now",
  async () => {
    const dir = mkdtempSync(join(tmpdir(), "scout-old-"));
    try {
      const old = join(dir, "linux-fixed-startup.py");
      writeFileSync(
        old,
        execFileSync(
          "git",
          ["show", "f8ab37bfb:scripts/dev-browser-observe/lib/linux-fixed-startup.py"],
          { cwd: repo },
        ),
      );
      const replay = async (...args) =>
        JSON.parse(
          (
            await run("python3", ["-I", "-B", fixture, ...args], {
              env: { ...process.env, CASES: "r4-replay" },
              timeout: 30_000,
            })
          ).stdout,
        );
      const before = await replay(old);
      assert.deepEqual(
        [before.status, before.error],
        ["failed", "startup role first seen terminal"],
      );
      assert.equal(before.published.length, 0);
      const after = await replay();
      assert.equal(after.status, "retained");
      assert.equal(after.published[0].worker.completedTransient.exitCode, 0);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  },
);
