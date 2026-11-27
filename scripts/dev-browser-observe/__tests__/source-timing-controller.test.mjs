import { createHash } from "node:crypto";
import assert from "node:assert/strict";
import {
  cpSync,
  mkdtempSync,
  writeFileSync,
  readFileSync,
  statSync,
  rmSync,
  existsSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, test, vi } from "vitest";
import { aggregate } from "../lib/aggregate.mjs";
import { execute, EXIT } from "../lib/run.mjs";
import { loadProfile } from "../lib/profile.mjs";
import { CSEC_EXE_SHA256, SOURCE_TIMING_PROTOCOL } from "../lib/source-timing-safety.mjs";
import * as util from "../lib/util.mjs";
import * as lock from "../lib/lock.mjs";
import * as identity from "../lib/identity.mjs";
import * as kinds from "../lib/fixture-kinds.mjs";
import { fakeFs, fakePage, fakeServer } from "./fakes.mjs";

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllEnvs();
});
test.each([
  "complete",
  "loader-trace",
  "partial-startup",
  "unclosed",
  "armed-overlap",
  "prewrite-pending",
])(
  "normal source-built controller: %s",
  async (mode) => {
    vi.stubEnv("NODE_PATH", undefined);
    vi.stubEnv("NODE_OPTIONS", undefined);
    if (mode === "loader-trace") vi.stubEnv("LD_DEBUG", "bindings");
    const dir = mkdtempSync(join(tmpdir(), "source-timing-control-")),
      exe = join(dir, "pinned-image");
    try {
      const p = JSON.parse(
        readFileSync(
          new URL(
            "../profiles/field-notes-direct-read-timing-abba-5000.example.json",
            import.meta.url,
          ),
        ),
      );
      p.sourceTimingSafety = SOURCE_TIMING_PROTOCOL;
      p.productSha = "6a4ec653fbb6cb7952b465632d5c58ca18f03e15";
      p.productDirtyIdentity = null;
      p.fixture.scale = 100;
      p.fixture.source.datasetSha256 =
        "fcaa222e52caaedb411a4136f106d4ddb5eaa7334a3ef03f34ccd282f6bab58a";
      p.fixture.dir = dir;
      p.scenarios = p.scenarios.slice(0, 1);
      p.buildReceipt = join(dir, "build.json");
      p.outDir = join(dir, "runs");
      p.limits.wallCeilingMs = 7200000;
      Object.assign(p.timeouts, { attemptMs: 60000, supplementalMs: 60000, restoreMs: 120000 });
      p.server.readyTimeoutMs = 300000;
      const pp = join(dir, "profile.json");
      writeFileSync(pp, JSON.stringify(p));
      writeFileSync(exe, "controlled nonexecutable");
      writeFileSync(
        p.buildReceipt,
        JSON.stringify({
          source: { sha: p.productSha, dirty: false },
          exe: { path: exe, sha256: CSEC_EXE_SHA256 },
          build: { profile: "dev", features: [], noDefaultFeatures: false },
          toolchain: { rustc: { release: "1.99.1" } },
          embeddedBytes: { checked: 64, missing: [] },
          cargoLockSha256: "804d5b831930816471fc101bd5c4ab390ccd1861d73808e046b4bfedb22c1dc4",
        }),
      );
      assert.equal(loadProfile(pp).profile.sourceTimingSafety, SOURCE_TIMING_PROTOCOL);
      vi.spyOn(identity, "collectIdentityFacts").mockReturnValue({});
      vi.spyOn(identity, "evaluateIdentity").mockReturnValue({ checks: [] });
      vi.spyOn(identity, "frozenIdentityChecks").mockReturnValue([]);
      vi.spyOn(lock, "acquireLock").mockResolvedValue({ release: async () => {} });
      const fs = fakeFs();
      const plan = fs.prepareAttempt;
      fs.prepareAttempt = (op) => plan({ ...op, article: "a" });
      vi.spyOn(kinds, "kindFor").mockReturnValue({
        loadManifest: () => ({}),
        adapterPath: () => exe,
        appDirs: () => [],
        adapterVariant: () => null,
        extraChecks: () => [],
        materialize: () => ({ appDir: dir, contentDir: dir }),
        receiptFixture: () => ({}),
        ops: () => fs,
      });
      const root = {
        pid: 555,
        ppid: 3,
        pgid: 555,
        bootId: "1edafe52-089e-4f16-a478-1143d29052bd",
        startTicks: "2",
        birthVersion: "linux-proc-v1",
      };
      const server = {
        ...fakeServer(),
        handle: { native: root, known: new Map([[555, root]]), exit: null },
        ready: Promise.resolve({
          baseUrl: "http://127.0.0.1:4000",
          readyNs: process.hrtime.bigint(),
        }),
        closeLog: async () => ({ pipes: "closed" }),
        startedNs: process.hrtime.bigint(),
        args: [],
        zfbEnv: {},
        envOverrides: {},
        removedDiagnosticEnv: [],
        diagnostic: false,
        sampleResources() {
          throw Error("legacy synchronous sample selected");
        },
        sampleResourcesAsync() {
          throw Error("legacy armed sampler selected");
        },
      };
      const deferred = Promise.withResolvers(),
        secondStarted = Promise.withResolvers();
      const pendingCase = mode === "armed-overlap" || mode === "prewrite-pending";
      let calls = 0,
        browserStarts = 0,
        active = 0,
        maxActive = 0;
      const file = statSync(exe, { bigint: true });
      const img = { path: exe, device: String(file.dev), inode: String(file.ino) };
      const sample = async () => {
        calls++;
        active++;
        maxActive = Math.max(maxActive, active);
        try {
          if (pendingCase && calls === 2) {
            secondStarted.resolve();
            return await deferred.promise;
          }
          const ns = process.hrtime.bigint(),
            t = String(ns);
          const helper = {
            // Each real sample spawns its own helper, and the evidence validator
            // requires distinct helper births; a session can span several ticks.
            pid: 1000 + calls,
            birth: { ...root, pid: 1000 + calls, startTicks: String(100 + calls) },
            spawnHrtimeNs: t,
            closed: true,
            pipesClosed: mode !== "unclosed",
            code: 0,
            signal: null,
          };
          return {
            helperResult: "complete",
            verifiedDiscoveries: [],
            helper,
            observedRssKiB: { 555: { VmRSS: 64, "rollup.Rss": 62 } },
            journalFailures: [],
            timing: {
              clock: {
                qualified: true,
                parent: {
                  platform: "linux",
                  clockId: "CLOCK_MONOTONIC",
                  bootId: root.bootId,
                  timeNamespace: "time:[1]",
                },
                helper: {
                  implementation: "clock_gettime(CLOCK_MONOTONIC)",
                  bootId: root.bootId,
                  timeNamespace: "time:[1]",
                },
              },
              parent: {
                entryNs: t,
                spawnCallNs: t,
                spawnReturnNs: t,
                exitCode: 0,
                exitSignal: null,
                deadlineNs: String(ns + 750000000n),
                resultNs: t,
                exitNs: t,
                closeNs: t,
                stdinCloseNs: t,
                stdoutCloseNs: t,
                stderrCloseNs: t,
              },
            },
            summedRssKiB: 64,
            processes: [
              {
                ...root,
                disposition: "live",
                rssKiB: 64,
                statusKiB: { VmRSS: 64 },
                smapsRollupKiB: mode === "partial-startup" ? null : { Rss: 62 },
                executable: img,
              },
            ],
          };
        } finally {
          active--;
        }
      };
      const page = fakePage({
        onWait(token, _, pg) {
          if (mode === "armed-overlap") return new Promise(() => {});
          return Promise.resolve({
            kind: "notify",
            t1: process.hrtime.bigint(),
            payload: { token, docUUID: pg.docUUID },
          });
        },
      });
      page.arm = async () => {
        if (mode === "prewrite-pending") await secondStarted.promise;
        return { armed: true, docUUID: "home", actual: "original body" };
      };
      let timer;
      const now = Number(process.hrtime.bigint() / 1000000n);
      const err = [];
      let launchClock;
      try {
        const code = await execute({
          profilePath: pp,
          label: "controlled",
          command: "run",
          supervisorId: "test",
          supervisorStartedMs: now,
          workDeadline: now + 7140000,
          supervisorDeadlineMs: now + 7200000,
          stderr: (x) => err.push(x),
          hooks: {
            admissionPath: join(dir, "active.json"),
            startDevServer: (args) => {
              if (mode === "complete") {
                launchClock.mockRestore();
                assert.equal(
                  args.readyTimeoutMs,
                  17,
                  "readiness allowance after executable digest",
                );
              }
              return server;
            },
            sourceTimingSample: sample,
            get sourceTimingExpectedDigest() {
              if (mode === "complete") {
                launchClock = vi
                  .spyOn(util, "hrtimeNs")
                  .mockReturnValue(BigInt(now + 7140000 - 17) * 1000000n);
              }
              return createHash("sha256").update("controlled nonexecutable").digest("hex");
            },
            launchOwnedBrowser: async (_, { signal }) => {
              browserStarts++;
              if (pendingCase)
                signal.addEventListener(
                  "abort",
                  () => {
                    timer = setImmediate(() =>
                      deferred.reject(
                        Object.assign(Error("late partial after overlap"), {
                          details: {
                            helper: { pid: 123, closed: false, pipesClosed: false },
                            observedRssKiB: { 555: { VmRSS: 71 } },
                          },
                        }),
                      ),
                    );
                  },
                  { once: true },
                );
              return { observer: page, identity: { engine: "controlled" } };
            },
            closeOwnedBrowser: async () => [],
            stopOwned: async () => ({ absent: true, remaining: [] }),
          },
        });
        const at = join(p.outDir, "controlled");
        const summary = JSON.parse(readFileSync(join(at, "summary.json")));
        const rows = readFileSync(join(at, "records.jsonl"), "utf8")
          .trim()
          .split("\n")
          .map(JSON.parse);
        if (mode === "loader-trace") {
          assert.equal(calls, 0);
          assert.equal(browserStarts, 0);
          assert.equal(fs.writes.length, 0);
          assert.notEqual(code, EXIT.ok);
          assert(JSON.stringify(rows).includes("source timing environment contains LD_DEBUG"));
          return;
        }
        assert.equal(
          maxActive,
          1,
          JSON.stringify({ err, issues: rows.filter((r) => r.error || r.type === "identity") }),
        );
        assert.equal(active, 0);
        assert.equal(
          code === EXIT.ok,
          mode === "complete",
          JSON.stringify({ code, err, summary, errors: rows.filter((r) => r.error) }),
        );
        if (mode === "complete") {
          writeFileSync(
            join(at, "supervisor-completion.json"),
            JSON.stringify({
              schema: "dev-browser-observe.supervisor-completion.v1",
              supervisorId: "test",
              exitCode: 0,
              sourceTimingSafety: SOURCE_TIMING_PROTOCOL,
              diagnostic: false,
              startedMonotonicMs: now,
              deadlineMonotonicMs: now + 7200000,
              controllerExitMonotonicMs: now + 50,
              controllerPid: root.ppid,
            }),
          );
          // The controlled launcher uses a nonexecutable fixture, so the formal
          // reducer must reject its real digest. Supply a synthetic qualified
          // receipt solely for the following workload-free reducer controls.
          const controlledReceipt = join(at, "receipt.json");
          const parsedReceipt = JSON.parse(readFileSync(controlledReceipt));
          const observe = () =>
            aggregate({ sessions: [{ label: "controlled", variant: "A", dir: at }] }).report;
          assert.equal(observe().complete, false);
          parsedReceipt.sourceTimingImageSha256 = CSEC_EXE_SHA256;
          writeFileSync(controlledReceipt, JSON.stringify(parsedReceipt));
          assert.equal(observe().complete, true, JSON.stringify(observe().sessions[0]));
          const legacy = join(p.outDir, "legacy");
          cpSync(at, legacy, { recursive: true });
          for (const name of ["receipt.json", "summary.json", "supervisor-completion.json"]) {
            const path = join(legacy, name),
              r = JSON.parse(readFileSync(path, "utf8"));
            delete r.sourceTimingSafety;
            if (r.profile) delete r.profile.sourceTimingSafety;
            writeFileSync(path, JSON.stringify(r));
          }
          const legacyRows = structuredClone(rows).filter(
            (r) => r.type !== "source-timing-rss-sample",
          );
          delete legacyRows[0].sourceTimingSafety;
          const writeRows = (dest, rs) =>
            writeFileSync(
              join(dest, "records.jsonl"),
              rs.map((r) => JSON.stringify(r)).join("\n") + "\n",
            );
          writeRows(legacy, legacyRows);
          const mixed = aggregate({
            sessions: [
              { label: "controlled", variant: "A", dir: at },
              { label: "legacy", variant: "A", dir: legacy },
            ],
          }).report;
          assert(!mixed.identity.ok && !mixed.pooledComparable);
          assert.deepEqual(mixed.pooled, {});
          for (const damage of [
            (r) => (r.find((x) => x.type === "attempt").t0Ns = "invalid"),
            (r) =>
              delete r.find((x) => x.type === "source-timing-rss-sample").result.timing.parent
                .stdinCloseNs,
            (r) =>
              (r.find(
                (x) => x.type === "source-timing-rss-sample",
              ).result.timing.clock.helper.bootId = "foreign"),
            (r) =>
              delete r.find((x) => x.type === "source-timing-rss-sample").result.observedRssKiB[555]
                .VmRSS,
            (r) =>
              (r.find((x) => x.type === "source-timing-rss-sample").result.processes[0].startTicks =
                "999"),
          ]) {
            const forged = structuredClone(rows);
            damage(forged);
            writeRows(at, forged);
            assert.equal(observe().sessions[0].timingExcluded, true);
          }
          const damaged = rows.filter((r) => r.type !== "source-timing-rss-sample");
          writeFileSync(
            join(at, "records.jsonl"),
            damaged.map((r) => JSON.stringify(r)).join("\n") + "\n",
          );
          assert.equal(observe().sessions[0].timingExcluded, true);
          assert.equal(summary.accounting.success, 12);
          assert.equal(fs.writes.length, 12);
          // Every gated decision goes through the shared settle-then-decide helper.
          const decisions = rows.filter((r) => r.type === "source-timing-decision");
          assert.deepEqual(
            decisions.map((d) => d.site),
            ["pre-batch", ...Array(12).fill("precheck"), "cleanup-entry"],
          );
          const writes = rows.filter((r) => r.type === "attempt");
          assert.equal(writes.length, 12);
          for (const a of writes) {
            assert.equal(a.sourceTimingDecision.site, "write");
            assert.equal(a.sourceTimingDecision.decisionNs, a.t0Ns);
          }
          const restored = rows.find((r) => r.type === "cleanup").sourceTimingDecision;
          assert.equal(restored.site, "restore");
          for (const d of [...decisions, ...writes.map((a) => a.sourceTimingDecision), restored]) {
            assert(["fresh", "settled"].includes(d.outcome), JSON.stringify(d));
            assert.equal(typeof d.pendingAtDecision, "boolean");
            assert.equal(typeof d.staleAtDecision, "boolean");
            assert(/^\d+$/.test(d.enteredNs) && BigInt(d.enteredNs) <= BigInt(d.decisionNs));
            assert(Number.isSafeInteger(d.settledEpoch) && d.settledEpoch >= 1);
            assert(/^\d+$/.test(d.waitedNs) && /^\d+$/.test(d.decisionNs));
          }
          assert.equal(summary.sourceTimingSafety.accepted, calls);
          assert.equal(summary.sourceTimingSafety.failure, null);
          assert.equal(rows.filter((x) => x.type === "attempt" && x.label === "warmup").length, 2);
        } else if (pendingCase) {
          assert.equal(calls, 2);
          assert.equal(fs.writes.length, mode === "prewrite-pending" ? 0 : 1);
          if (mode === "prewrite-pending")
            assert(rows.some((r) => r.phase === "before-mutation" && r.writePerformed === false));
          assert.equal(fs.restores, 0);
          assert.equal(summary.sourceTimingSafety.peakFailedObservedSubtotalKiB, 71);
          assert.match(
            summary.sourceTimingSafety.failure.error,
            // The write now waits for the in-flight sample (#4878 owner decision);
            // one that never settles refuses at its core+closure bound.
            mode === "armed-overlap" ? /overlap/ : /never-settles/,
          );
          const marker = JSON.parse(readFileSync(join(dir, "active.json")));
          assert(marker.unresolved.some((r) => r.helper?.pid === 123));
        } else {
          assert.equal(browserStarts, 0);
          assert.equal(fs.writes.length, 0);
          assert.equal(summary.sourceTimingSafety.accepted, 0);
          assert.equal(existsSync(join(dir, "active.json")), mode === "unclosed");
        }
      } finally {
        clearImmediate(timer);
      }
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  },
  7000,
);
