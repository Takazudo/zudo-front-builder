import assert from "node:assert/strict";
import { EventEmitter } from "node:events";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  DurableSink,
  createBarrierPoint,
  createOwnershipJournal,
  createReceiptWriter,
} from "../lib/durable-io.mjs";
import { blankBrowserAudit } from "../lib/fixed-family-admission.mjs";
import { startFixedRssSafety } from "../lib/fixed-rss-safety.mjs";
import { Recorder } from "../lib/recorder.mjs";
import { deps, fakeFs, fakePage } from "./fakes.mjs";

// Real-timer replay harness for the durability windows (design §4): the real
// startFixedRssSafety with a 1000 ms setInterval and a fake sample that spawns its
// helper synchronously and settles in 52 ms. Like linux-rss-sample.mjs:90, the
// fake refuses a result the parent observes after the helper's 750 ms deadline.
export const MS = 1_000_000n;
export const SETTLE_MS = 52;

export const identity = (pid, ppid, pgid) => ({
  pid,
  ppid,
  pgid,
  startTicks: String(pid),
  bootId: "boot",
  birthVersion: "linux-proc-v1",
  state: "S",
});
export const root = identity(3, 2, 3),
  dev = identity(4, 3, 3),
  worker = identity(5, 4, 5);

export const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const immediate = () => new Promise((resolve) => setImmediate(resolve));

// One controller: records, registry and receipt writers in either mode.
export function controller(durable) {
  const dir = mkdtempSync(join(tmpdir(), "dbo-window-"));
  const sink = durable ? new DurableSink() : null;
  const rec = new Recorder(join(dir, "records.jsonl"), null, sink);
  const registry = createOwnershipJournal(join(dir, "owned.jsonl"), { sink });
  const receipt = createReceiptWriter(join(dir, "receipt.json"), { sink });
  const barrier = sink
    ? createBarrierPoint({
        sink,
        recorder: rec,
        deadlineNs: process.hrtime.bigint() + 60_000n * MS,
      })
    : null;
  return {
    dir,
    sink,
    rec,
    registry,
    receipt,
    barrier,
    close() {
      sink?.close();
      rmSync(dir, { recursive: true, force: true });
    },
  };
}

export function sampler(c, { onDispatch = () => {}, known = [root, dev, worker] } = {}) {
  const settled = [];
  const failures = [];
  const handle = { native: { pid: 3 }, known: new Map(known.map((p) => [p.pid, p])) };
  handle.onNewOwned = (p) => c.registry.append({ type: "owned", ...p });
  let dispatches = 0;
  const monitor = startFixedRssSafety({
    joinOwnedFamily: true,
    server: { handle },
    fixedExecutables: { protocol: "linux-fixed-zfb.v4", root: { path: "/node" } },
    limits: { rssCeilingKiB: 4194304, sampleEveryMs: 1000 },
    append: (r) => c.rec.append(r),
    abort: (error) => failures.push(error),
    custody: {
      familyProof: () => ({
        root,
        dev,
        worker,
        terminal: { epoch: 1 },
        release: { receipt: { epoch: 1 } },
      }),
    },
    sample(_server, _path, { onHelperSpawn }) {
      dispatches++;
      const done = Promise.withResolvers();
      const now = process.hrtime.bigint();
      const helper = {
        pid: 100 + dispatches,
        entryNs: String(now),
        spawnHrtimeNs: String(now),
        deadlineNs: String(now + 750n * MS),
      };
      onHelperSpawn(helper);
      onDispatch(dispatches, handle);
      setTimeout(() => {
        const observed = process.hrtime.bigint();
        if (observed > BigInt(helper.deadlineNs))
          return done.reject(Error("sampling helper deadline reached"));
        done.resolve({
          summedRssKiB: 174772,
          helperResult: "complete",
          processes: [root, dev].map((p) => ({ ...p, disposition: "live" })),
          startupEpochCoverage: { complete: true },
          helper: { ...helper, closed: true, pipesClosed: true },
          timing: {
            clock: { qualified: true },
            parent: {
              entryNs: helper.entryNs,
              deadlineNs: helper.deadlineNs,
              resultNs: String(observed),
            },
          },
        });
      }, SETTLE_MS);
      settled.push(done.promise);
      return done.promise;
    },
  });
  return {
    monitor,
    handle,
    failures,
    settled,
    get dispatches() {
      return dispatches;
    },
  };
}

// Waits until `epoch` is accepted and returns its start L.
export async function acceptedEpoch(s, epoch) {
  while (s.settled.length < epoch) await sleep(20);
  await s.settled[epoch - 1].catch(() => {});
  await immediate();
  const accepted = s.monitor.acceptedSample();
  assert.equal(accepted?.epoch, epoch);
  return BigInt(accepted.startedNs);
}

export function familyBatch(c, s, { snapshot } = {}) {
  const audit = blankBrowserAudit(),
    ctx = new EventEmitter();
  const page0 = Object.assign(new EventEmitter(), { url: () => "about:blank" });
  ctx.pages = () => [];
  audit.attach(ctx);
  ctx.pages = () => [page0];
  const input = {
    scenarios: [{ id: "family-admission-only", ops: [] }],
    deps: {
      ...deps({ page: fakePage(), fs: audit.trackFixture(fakeFs()), recorder: c.rec }),
      ...(c.barrier && { durability: { barrier: c.barrier } }),
    },
  };
  const options = {
    strategy: "blank-join-owned.v2",
    safety: s.monitor,
    browserAudit: audit,
    snapshot: snapshot ?? (() => [root, dev]),
    known: s.handle.known,
    verifyImages() {},
    deadlineNs: process.hrtime.bigint() + 60_000n * MS,
    signal: new AbortController().signal,
    append: (r) => c.rec.append(r),
  };
  return { input, options };
}

export async function outcomeOf(promise) {
  try {
    return { value: await promise };
  } catch (error) {
    return { error };
  }
}
