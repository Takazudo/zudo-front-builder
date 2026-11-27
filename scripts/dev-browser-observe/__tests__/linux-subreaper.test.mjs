import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { existsSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";
import { test } from "vitest";

const run = promisify(execFile);
const node26 = join(homedir(), ".nvm/versions/node/v26.13.1/bin/node");
const script = fileURLToPath(new URL("./linux-subreaper-escape.mjs", import.meta.url));
const available = process.platform === "linux" && existsSync(node26);

// The fixed v4 runs use that pinned Node; hosts without it (CI) skip this case.
test.runIf(available)(
  "an escaped descendant reparents to the subreaper controller's census",
  async () => {
    const census = async (mode) =>
      JSON.parse((await run(node26, ["--no-warnings", script, mode], { timeout: 20_000 })).stdout);
    const withReaper = await census("subreaper");
    assert.equal(withReaper.enabled.enabled, true);
    assert.deepEqual(withReaper.unknown, [withReaper.escaped]);
    const without = await census("plain");
    assert(!without.children.includes(without.escaped), "without a subreaper init adopts it");
    const cleared = await census("cleared");
    assert(!cleared.children.includes(cleared.escaped), "a cleared subreaper adopts nothing");
  },
);

// review-2 finding 1: the root shim exits before zfb, zfb is reparented here and
// dies under SIGKILL as a zombie; the stop must still report the group absent, so
// run.mjs writes no cleanup-unresolved marker.
test.runIf(available)("a SIGKILL stop after the shim exits first is absent", async () => {
  const stop = fileURLToPath(new URL("./linux-subreaper-stop.mjs", import.meta.url));
  const r = JSON.parse((await run(node26, ["--no-warnings", stop], { timeout: 20_000 })).stdout);
  assert.deepEqual(r.childNow, { state: "Z", ppid: r.controller }, JSON.stringify(r));
  assert.equal(r.absent, true, JSON.stringify(r));
  assert.deepEqual(r.remaining, []);
  assert(r.endings.some((e) => e.pid === r.child && e.adoptedZombie && e.endedAfter === "SIGKILL"));
  assert(r.steps.some(([signal, target]) => signal === "SIGKILL" && target === r.child));
});
