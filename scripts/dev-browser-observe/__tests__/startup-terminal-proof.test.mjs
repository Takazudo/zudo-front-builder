import assert from "node:assert/strict";
import { test } from "vitest";
import { startupTerminalObservation } from "../lib/fixed-rss-custody.mjs";

test("a startup role needs a later independent ordinary original-pair terminal observation", () => {
  const birth = {
    pid: 31,
    ppid: 29,
    pgid: 31,
    bootId: "boot",
    startTicks: "230",
    birthVersion: "linux-proc-v1",
  };
  const cycle = { startupPollPid: 31, epoch: 2, session: "session" };
  const receipt = { session: "session", epoch: 2 };
  const p = {
    ...birth,
    startedNs: "201",
    disposition: "custodied-departure",
    rssKiB: null,
    statusKiB: null,
    smapsRollupKiB: null,
    custodyReceipt: receipt,
  };
  const check = (row, config = cycle) => startupTerminalObservation([row], config, birth, "200");
  assert.equal(check(p), p);
  for (const delta of [
    { disposition: "live" },
    { disposition: "proved-departure" },
    { disposition: "retained-terminal-proof" },
    { startedNs: "199" },
    { rssKiB: 0 },
    { statusKiB: 0 },
    { smapsRollupKiB: {} },
    { pgid: 29 },
    { ppid: 30 },
    { startTicks: "229" },
    { custodyReceipt: { ...receipt, epoch: 1 } },
    { custodyReceipt: { ...receipt, session: "foreign" } },
  ]) {
    assert.equal(check({ ...p, ...delta }), null);
  }
  assert.equal(check(p, { ...cycle, startupPollPid: 30 }), null);
  assert.equal(check(p, { epoch: 2, session: "session" }), null);
});
