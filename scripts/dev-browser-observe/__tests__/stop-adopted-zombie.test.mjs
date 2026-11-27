import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { readFileSync } from "node:fs";
import { test } from "vitest";
import { linuxThreads } from "../lib/linux-birth.mjs";
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
async function stop(zombieParent, recordedParent = root.pid, threads = 1) {
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
      threadsImpl: (pid) => (pid === zfb.pid ? threads : null),
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

// review-3 finding 2: a leader that called pthread_exit reads Z while a thread runs.
test.each([
  ["a Z leader with a live thread", 2],
  ["a Z leader whose thread count is unreadable", null],
])("%s is not ended", async (_label, threads) => {
  const { report, signals } = await stop(CONTROLLER, root.pid, threads);
  assert.equal(report.absent, false);
  assert.deepEqual(
    report.remaining.map((p) => p.pid),
    [501],
  );
  assert(!report.endings.some((e) => e.pid === 501));
  assert(signals.some(([pid, signal]) => pid === 501 && signal === "SIGKILL"));
});

// The kernel field linuxThreads reads: a fully dead zombie is 1, a Z leader with a live thread is 2.
test.runIf(process.platform === "linux")(
  "linuxThreads tells a dead zombie from a live-thread Z leader",
  async () => {
    const child = spawn(
      "python3",
      [
        "-c",
        [
          "import ctypes, os, sys, threading, time",
          "a = os.fork()",
          "if a == 0: os._exit(0)",
          "b = os.fork()",
          "if b == 0:",
          "    threading.Thread(target=time.sleep, args=(20,)).start()",
          "    ctypes.CDLL(None).pthread_exit(None)",
          "print(a, b, flush=True)",
          "sys.stdin.read()",
          "os.kill(b, 9)",
        ].join("\n"),
      ],
      { stdio: ["pipe", "pipe", "inherit"] },
    );
    try {
      const [dead, leader] = (await new Promise((r) => child.stdout.once("data", r)))
        .toString()
        .trim()
        .split(" ")
        .map(Number);
      const state = (pid) => readFileSync(`/proc/${pid}/stat`, "utf8").split(") ")[1][0];
      const deadline = Date.now() + 5000;
      while ((state(dead) !== "Z" || state(leader) !== "Z") && Date.now() < deadline)
        await new Promise((r) => setTimeout(r, 10));
      assert.equal(state(dead), "Z");
      assert.equal(state(leader), "Z");
      assert.equal(linuxThreads(dead), 1);
      assert.equal(linuxThreads(leader), 2);
    } finally {
      child.stdin.end();
      await new Promise((r) => child.once("exit", r));
    }
  },
);
