import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";
import { test } from "vitest";

const run = promisify(execFile);
const fixture = fileURLToPath(new URL("./startup-holder-e2e.py", import.meta.url));

// The holder's real serve() in startup mode against a real process cell (#4862 M6c).
test.runIf(process.platform === "linux")(
  "the holder retains live and zombie workers, settles an exit, and reports completion",
  async () => {
    const { stdout } = await run("python3", ["-I", "-B", fixture], { timeout: 60_000 });
    assert.match(stdout.trim(), /^PASS holder-live-custody /);
  },
);
