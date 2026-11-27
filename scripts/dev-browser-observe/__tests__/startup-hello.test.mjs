import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { test } from "vitest";
import { EventEmitter } from "node:events";
import { sampleRendererMemory, validateStartupHelloRow } from "../lib/linux-rss-sample.mjs";
import { registerStartupBirth, startupTerminalObservation } from "../lib/fixed-rss-custody.mjs";
const root = {
  pid: 10,
  ppid: 9,
  pgid: 10,
  startTicks: "20",
  bootId: "11111111-2222-3333-4444-555555555555",
  birthVersion: "linux-proc-v1",
};
const dev = { ...root, pid: 11, ppid: 10, startTicks: "21" };
const birth = { ...root, pid: 12, ppid: 11, pgid: 12, startTicks: "22" };
const descriptors = [
  ["5", "255"],
  ["61", "321"],
];
const cycle = {
  allowStartupAddition: true,
  session: "session",
  epoch: 1,
  holderPid: 20,
  tokens: {},
};
const receipt = {
  session: "session",
  epoch: 1,
  holderPid: 20,
  helperPid: 21,
  verifiedNs: "250",
  descriptors,
};
const a = { root, dev, birth, token: "ac".repeat(16), descriptors, acquiredNs: "230", receipt };
test("a hello addition stays distinct from the immutable request and later official epoch", () => {
  const check = (item = a, request = cycle) =>
    validateStartupHelloRow(item, request, { pid: 21 }, root, "200", "300");
  check();
  for (const change of [
    (r) => (r.receipt.session = "other"),
    (r) => (r.receipt.helperPid = 20),
    (r) => (r.receipt.epoch = 2),
    (r) => (r.birth.pgid = 10),
    (r) => (r.dev.ppid = 9),
    (r) => (r.root.startTicks = "19"),
    (r) => (r.acquiredNs = "251"),
    (r) => (r.receipt.verifiedNs = "301"),
    (r) => (r.token = "bad"),
    (r) =>
      (r.receipt.descriptors = [
        ["5", "256"],
        ["61", "321"],
      ]),
  ]) {
    const copy = structuredClone(a);
    change(copy);
    assert.throws(() => check(copy));
  }
  for (const config of [
    { ...cycle, allowStartupAddition: false },
    { ...cycle, startupPollPid: 12 },
    { ...cycle, tokens: { 12: a.token } },
  ])
    assert.throws(() => check(a, config));
  const observed = {
    ...birth,
    disposition: "custodied-departure",
    fromStartupHello: true,
    custodyReceipt: receipt,
    startedNs: "225",
    rssKiB: null,
    statusKiB: null,
    smapsRollupKiB: null,
  };
  assert.equal(startupTerminalObservation([observed], cycle, birth, "230"), null);
});
test("the normal Python sample measures live additions and authenticates terminal additions", () => {
  const fixture = fileURLToPath(new URL("./startup-hello-control.py", import.meta.url));
  assert.equal(execFileSync("python3", ["-I", "-B", fixture], { encoding: "utf8" }).trim(), "PASS");
});

test.each(["live", "terminal", "missing", "uncorroborated"])(
  "the actual controller parser accounts for a %s startup hello addition",
  async (mode) => {
    const n = () => String(process.hrtime.bigint());
    const launch = { ...root, state: "S", comm: "node", ppid: process.pid, argv: ["/shim", "dev"] };
    const native = { ...dev, state: "S", comm: "zfb" };
    const worker = { ...birth, state: "S", comm: "zfb" };
    const image = { path: "/node", device: "1", inode: "100" };
    const elf = { path: "/zfb", device: "1", inode: "101" };
    const helper = Object.assign(new EventEmitter(), {
      pid: receipt.helperPid,
      stdin: new EventEmitter(),
      stdout: new EventEmitter(),
      stderr: new EventEmitter(),
      kill() {},
      unref() {},
    });
    let request;
    helper.stdin.end = (raw) => {
      request = JSON.parse(raw);
    };
    const handle = { native: launch, known: new Map([[launch.pid, launch]]), onNewOwned() {} };
    const custody = {
      begin: () => ({ ...cycle }),
      validateStartupHello(value, config, realHelper) {
        assert.deepEqual(config.tokens, {});
        assert.equal(realHelper.pid, helper.pid);
        assert.equal(value.token, a.token);
        if (mode === "uncorroborated")
          throw Error("startup hello contradicts independent holder publication");
      },
      matches: (p, token) => p.pid === worker.pid && token === a.token,
      sameDescriptors: (p) =>
        JSON.stringify(p.custodyReceipt.descriptors) === JSON.stringify(descriptors),
      selectStartupCycle(processes, releases, config) {
        assert.deepEqual(releases, []);
        assert.equal(startupTerminalObservation(processes, config, worker, "0"), null);
        return null;
      },
    };
    let dispatched;
    const result = sampleRendererMemory({ handle, isAlive: () => ({ alive: true }) }, image.path, {
      onHelperSpawn: (captured) => {
        dispatched = captured;
      },
      launch: () => helper,
      fixedExecutables: { protocol: "linux-fixed-zfb.v4", root: image, child: elf, launch: {} },
      custody,
    });
    assert.deepEqual(
      request.known.map((p) => p.pid),
      [launch.pid],
    );
    registerStartupBirth(handle, worker);
    let seq = 0;
    const timing = (stage, count, extra = {}) => ({
      type: "timing",
      version: "linux-rss-timing.v1",
      seq: seq++,
      stage,
      ns: Array.from({ length: count }, n),
      ...extra,
    });
    const rows = [
      timing("request", 4, {
        clock: {
          implementation: "clock_gettime(CLOCK_MONOTONIC)",
          bootId: root.bootId,
          timeNamespace: null,
        },
      }),
    ];
    const helperStart = n();
    const actualAddition = {
      ...a,
      root: launch,
      dev: native,
      birth: worker,
      acquiredNs: n(),
      receipt: { ...receipt, verifiedNs: n() },
    };
    rows.push(
      { type: "helper", birth: { ...launch, pid: helper.pid, pgid: helper.pid } },
      { type: "executable", pathInput: image.path, executable: image },
      { type: "startup-addition", addition: actualAddition },
    );
    const bound = (p) => ({
      type: "bound",
      birth: p,
      binding: { pidfdFlags: 0, fdinfoPid: p.pid, verifiedNs: n() },
    });
    rows.push(bound(launch), bound(native));
    const live = (p, executable, rss, extra = {}) => {
      const start = n();
      for (const [source, v] of [
        ["VmRSS", rss - 1],
        ["rollup.Rss", rss],
      ])
        rows.push({ type: "rss", source, birth: p, rssKiB: v, observedNs: n() });
      rows.push(
        {
          type: "observation",
          process: {
            ...p,
            disposition: "live",
            rssKiB: rss,
            statusKiB: { VmRSS: rss - 1 },
            smapsRollupKiB: { Rss: rss },
            threads: { listed: 1, rendererHosts: [] },
            executable,
            startedNs: start,
            endNs: n(),
            ...extra,
          },
        },
        timing("observationOutput", 4, { pid: p.pid }),
      );
    };
    live(native, elf, 75);
    live(launch, image, 50);
    if (mode === "live") {
      rows.push(bound(worker));
      live(worker, { ...elf, workerArg: "__zfb-compiler-worker" }, 25, { fromStartupHello: true });
    } else if (mode === "terminal") {
      rows.push(
        {
          type: "observation",
          process: {
            ...worker,
            fromStartupHello: true,
            disposition: "custodied-departure",
            terminalProof: { birth: worker, pidfdFlags: 0, pollMask: 17, observedNs: n() },
            custodyToken: a.token,
            custodyReceipt: actualAddition.receipt,
            rssKiB: null,
            statusKiB: null,
            smapsRollupKiB: null,
            threads: null,
            executable: null,
            startedNs: helperStart,
            endNs: n(),
          },
        },
        timing("observationOutput", 4, { pid: worker.pid }),
      );
    }
    rows.push(
      timing("finalRootBegin", 1),
      timing("finalRoot", 2),
      timing("cleanupBegin", 1),
      timing("fdCleanup", 2),
      { type: "complete", readCalls: 1, procInputBytes: 64, helperMaxRssKiB: 100 },
      timing("terminalOutput", 6),
      timing("preExit", 1),
    );
    helper.emit("spawn");
    helper.stdin.emit("finish");
    helper.stdout.emit(
      "data",
      Buffer.from(rows.map((row) => JSON.stringify(row)).join("\n") + "\n"),
    );
    for (const pipe of [helper.stdin, helper.stdout, helper.stderr]) pipe.emit("close");
    helper.emit("exit", 0, null);
    helper.emit("close", 0, null);
    if (mode === "uncorroborated") {
      await assert.rejects(
        result,
        (e) =>
          e.details.error.includes("independent holder publication") &&
          e.details.summedRssKiB === null,
      );
      return;
    }
    if (mode === "missing") {
      await assert.rejects(
        result,
        (e) =>
          e.details.summedRssKiB === null &&
          e.details.observedSubtotalKiB === 125 &&
          e.details.startupEpochCoverage.missing[0].birth.pid === worker.pid,
      );
      return;
    }
    const record = await result;
    assert.equal(record.summedRssKiB, mode === "live" ? 150 : 125);
    assert.deepEqual(record.startupEpochCoverage, { complete: true, missing: [] });
    assert.equal(record.processes.at(-1).rssKiB, mode === "live" ? 25 : null);
    assert.equal(record.helper.closed, true);
    assert.deepEqual(dispatched, {
      pid: record.helper.pid,
      spawnHrtimeNs: record.helper.spawnHrtimeNs,
      entryNs: record.timing.parent.entryNs,
      deadlineNs: record.timing.parent.deadlineNs,
    });
  },
);
