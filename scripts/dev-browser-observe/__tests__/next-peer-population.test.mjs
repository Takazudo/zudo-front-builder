import assert from "node:assert/strict";
import { test } from "vitest";
import { reconcileNextPopulation } from "../lib/next-peer-population.mjs";

const role = (pid, ppid) => ({
  pid,
  ppid,
  pgid: 21,
  startTicks: String(100 + pid),
  bootId: "1edafe52-089e-4f16-a478-1143d29052bd",
  birthVersion: "linux-proc-v1",
  state: "S",
});
function fixture() {
  const chain = [role(21, 11), role(22, 21), role(23, 22)],
    worker = role(24, 23);
  const all = [...chain, worker],
    terminalProof = { birth: worker, pidfdFlags: 0, pollMask: 17, observedNs: "123" };
  const stop = {
    absent: true,
    remaining: [],
    known: [...all],
    ownedBefore: [...chain],
    rootAlive: true,
    exit: { exitCode: 0, signalCode: null },
    steps: [{ signal: "SIGINT", target: "group:21", delivery: "sent" }],
    endings: all.map((r) => ({
      pid: r.pid,
      endedAfter: r.pid === worker.pid ? "before-stop" : "SIGINT",
      ...(r.pid === 21 && { exit: { exitCode: 0, signalCode: null } }),
    })),
  };
  const registry = all
    .map((b, i) => ({ ...b, type: i ? "owned" : "spawn" }))
    .concat([{ type: "stopped", pid: 21, absent: true }]);
  return { chain, worker, terminalProof, registry, stop };
}
test("a complete post-stop census matches all qualified births and the original group terminal", () => {
  assert.equal(reconcileNextPopulation(fixture()).ok, true);
});
test("clean teardown cannot qualify an unknown or changed population, omitted registry evidence, or contradictory exit", () => {
  const changes = [
    (x) => x.stop.known.push(role(25, 23)),
    (x) => x.stop.ownedBefore.push(role(25, 23)),
    (x) => x.registry.push({ ...role(25, 23), type: "owned" }),
    (x) => x.stop.endings.push({ pid: 25 }),
    (x) => x.stop.endings.pop(),
    (x) => x.registry.splice(1, 1),
    (x) => (x.stop.known[1] = { ...x.stop.known[1], startTicks: "999" }),
    (x) => x.stop.ownedBefore.push(x.worker),
    (x) => (x.terminalProof = null),
    (x) => x.stop.remaining.push(x.worker),
    (x) => x.registry.pop(),
  ];
  for (const change of changes) {
    const x = fixture();
    change(x);
    const report = reconcileNextPopulation(x);
    assert.equal(report.resourceAbsent, true);
    assert.equal(report.ok, false);
    assert(report.violations.length > 0);
  }
});

test("persistent roles must be live at the pre-stop census and every ending has a consistent stage", () => {
  const changes = [
    (x) => x.stop.ownedBefore.pop(),
    (x) => x.stop.ownedBefore.splice(0),
    (x) => (x.stop.ownedBefore[0] = { ...x.chain[0], state: "Z" }),
    (x) => (x.stop.rootAlive = false),
    (x) => (x.stop.endings[1].endedAfter = "never"),
    (x) => delete x.stop.endings[1].endedAfter,
    (x) => (x.stop.endings[0].endedAfter = "before-stop"),
    (x) => (x.stop.endings.at(-1).endedAfter = "SIGINT"),
    (x) => (x.stop.endings[2].startTicks = "wrong"),
    (x) => x.stop.steps.splice(0),
    (x) => (x.stop.endings[0].exit = { exitCode: 7 }),
  ];
  for (const change of changes) {
    const x = fixture();
    change(x);
    const r = reconcileNextPopulation(x);
    assert.equal(r.resourceAbsent, true);
    assert.equal(r.ok, false);
  }
});
