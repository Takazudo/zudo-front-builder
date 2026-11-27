import assert from "node:assert/strict";
import { test } from "vitest";
import { timeStartupSelection } from "../lib/linux-rss-sample.mjs";
import {
  requireStartupRelease,
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
