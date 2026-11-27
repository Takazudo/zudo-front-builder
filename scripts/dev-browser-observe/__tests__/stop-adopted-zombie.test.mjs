import assert from "node:assert/strict";
import { test } from "vitest";
import { stopOwned } from "../lib/process-owner.mjs";

// stopLinux on recorded births: the root shim has exited and zfb is now a zombie.
const CONTROLLER = 1000;
const birth = (pid, ppid, pgid, state = "S") => ({
  pid,
  ppid,
  pgid,
  state,
  startTicks: String(pid),
  bootId: "8116e36c-eeea-4ca0-973f-b1af4f6144d6",
  birthVersion: "linux-proc-v1",
});
const root = birth(500, CONTROLLER, 500);
async function stop(zombieParent, recordedParent = root.pid) {
  const zfb = birth(501, recordedParent, 500);
  const handle = {
    native: root,
    exit: { exitCode: 0, signalCode: null },
    known: new Map([
      [root.pid, root],
      [zfb.pid, zfb],
    ]),
  };
  const now = new Map([[zfb.pid, { ...zfb, ppid: zombieParent, state: "Z" }]]);
  const signals = [];
  return {
    report: await stopOwned(handle, {
      platform: "linux",
      intWaitMs: 0,
      termWaitMs: 0,
      killWaitMs: 0,
      controllerPid: CONTROLLER,
      psImpl: () => "",
      birthImpl: (pid) => now.get(pid) ?? null,
      killImpl: (...a) => signals.push(a),
    }),
    signals,
  };
}

test("a zombie reparented to the subreaper controller counts as ended", async () => {
  const { report } = await stop(CONTROLLER);
  assert.equal(report.absent, true);
  assert.deepEqual(report.remaining, []);
  assert.deepEqual(
    report.endings.find((e) => e.pid === 501),
    { pid: 501, endedAfter: "before-stop", adoptedZombie: true },
  );
});

test.each([
  ["a zombie of another parent", 1, undefined],
  ["the controller's own libuv child as a zombie", CONTROLLER, CONTROLLER],
])("%s is still left for its reaper", async (_label, parent, recorded) => {
  const { report, signals } = await stop(parent, recorded);
  assert.equal(report.absent, false);
  assert.deepEqual(
    report.remaining.map((p) => p.pid),
    [501],
  );
  assert(signals.some(([pid, signal]) => pid === 501 && signal === "SIGKILL"));
});
