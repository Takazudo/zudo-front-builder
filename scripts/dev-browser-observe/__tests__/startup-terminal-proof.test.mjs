import assert from "node:assert/strict";
import { test } from "vitest";
import { startupEpochCoverage, timeStartupSelection } from "../lib/linux-rss-sample.mjs";
import {
  requireStartupRelease,
  registerStartupBirth,
  startupReleaseObservation,
  startupTerminalObservation,
} from "../lib/fixed-rss-custody.mjs";

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

test("startup readiness requires a later distinct closed epoch releasing the same original pair", () => {
  const birth = {
    pid: 30,
    ppid: 22,
    pgid: 30,
    bootId: "boot",
    startTicks: "100",
    birthVersion: "linux-proc-v1",
  };
  const desc = [
    ["5", "64"],
    ["61", "842"],
  ];
  const first = { epoch: 2, session: "session", holderPid: 40, helperPid: 41, descriptors: desc };
  const terminal = {
    ...birth,
    custodyReceipt: first,
    custodyToken: "token",
    terminalProof: { observedNs: "400" },
  };
  const cycle = { epoch: 3, session: "session" };
  const release = {
    birth,
    token: "token",
    receipt: { ...first, epoch: 3, helperPid: 42, verifiedNs: "500" },
  };
  assert.equal(startupReleaseObservation([release], cycle, terminal), release);
  assert.equal(startupReleaseObservation([], cycle, terminal), null);
  assert.throws(() => requireStartupRelease("retained", terminal, null));
  requireStartupRelease("retained", terminal, release);
  for (const delta of [
    { epoch: 2 },
    { epoch: 4 },
    { helperPid: 41 },
    { holderPid: 39 },
    { session: "other" },
    { verifiedNs: "399" },
    { descriptors: [desc[0], ["61", "843"]] },
  ])
    assert.throws(() =>
      startupReleaseObservation(
        [{ ...release, receipt: { ...release.receipt, ...delta } }],
        cycle,
        terminal,
      ),
    );
  assert.throws(() => startupReleaseObservation([{ ...release, token: "wrong" }], cycle, terminal));
  assert.throws(() => startupReleaseObservation([release, release], cycle, terminal));
});

test("startup validation completes within the core even when selecting a receipt takes time", () => {
  let at = 99n;
  const select = () => {
    at += 2n;
    return { terminal: true };
  };
  assert.deepEqual(
    timeStartupSelection(select, () => String(at), 100n),
    {
      resultNs: "101",
      error: "sampling helper deadline reached",
      selection: null,
    },
  );
  assert.deepEqual(
    timeStartupSelection(
      () => {
        throw Error("bad release");
      },
      () => String(at),
      200n,
    ),
    {
      resultNs: "101",
      error: "bad release",
      selection: null,
    },
  );
  assert.deepEqual(
    timeStartupSelection(select, () => String(at), 103n),
    {
      resultNs: "103",
      error: null,
      selection: { terminal: true },
    },
  );
});

test("a discovered private group remains known for immediate abort before another epoch", () => {
  const b = {
    pid: 77,
    ppid: 75,
    pgid: 77,
    startTicks: "140",
    bootId: "boot",
    birthVersion: "linux-proc-v1",
  };
  const handle = {
    known: new Map(),
    onNewOwned: () => {
      throw Error("journal failure");
    },
  };
  assert.throws(() => registerStartupBirth(handle, b), /journal failure/);
  assert.equal(handle.known.get(77).pgid, 77);
  registerStartupBirth(handle, b);
  assert.throws(() => registerStartupBirth(handle, { ...b, startTicks: "141" }), /conflicts/);
});

test("a pre-seed epoch cannot claim or lose a concurrent cleanup-only startup birth", () => {
  const root = { pid: 10, pgid: 10 },
    worker = {
      pid: 31,
      pgid: 31,
      ppid: 29,
      source: "fixed-startup-scout",
      bootId: "boot",
      startTicks: "40",
      birthVersion: "linux-proc-v1",
    };
  const known = new Map([
      [10, root],
      [31, worker],
    ]),
    bound = new Map([[10, root]]);
  const old = startupEpochCoverage(known, [root], bound, new Set([10]));
  assert.equal(old.complete, false);
  assert.equal(old.missing[0].birth.pid, 31);
  assert.equal(old.missing[0].rssKiB, null);
  assert.equal(old.missing[0].source, "fixed-startup-scout");
  assert.equal(startupEpochCoverage(known, [root, worker], bound, new Set([10])).complete, false);
  bound.set(31, worker);
  assert.equal(startupEpochCoverage(known, [root], bound, new Set([10])).complete, false);
  assert.deepEqual(startupEpochCoverage(known, [root], bound, new Set([10, 31])), {
    complete: true,
    missing: [],
  });
  bound.delete(31);
  known.set(50, { pid: 50, source: "unknown" });
  assert.equal(startupEpochCoverage(known, [root], bound, new Set([10])).complete, false);
});
