import assert from "node:assert/strict";
import { execFile, spawn } from "node:child_process";
import { once } from "node:events";
import { appendFileSync, existsSync, readlinkSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";
import { test } from "vitest";
import { linuxBirth } from "../lib/linux-birth.mjs";
import { sampleRendererMemory } from "../lib/linux-rss-sample.mjs";
import { ownedProcesses } from "../lib/process-owner.mjs";

const linux = test.skipIf(process.platform !== "linux");
const fixture = fileURLToPath(new URL("./linux-rss-fixture.py", import.meta.url));

// Optional raw receipts for the bounded fixture run, outside the repository.
function receipt(data) {
  if (process.env.ZFB_RSS_FIXTURE_RECEIPTS)
    appendFileSync(process.env.ZFB_RSS_FIXTURE_RECEIPTS, JSON.stringify(data) + "\n");
}

async function fixtureSample(label, server, ...args) {
  let record;
  try {
    record = await sampleRendererMemory(server, ...args);
    return record;
  } catch (error) {
    record = error.details;
    throw error;
  } finally {
    if (record)
      receipt({
        control: label,
        root: server.handle.native.pid,
        helperAbsent: record.helper?.pid ? !existsSync(`/proc/${record.helper.pid}`) : null,
        record,
      });
  }
}

linux(
  "group pidfd, proc-dir races, partial RSS, identity and cleanup on controlled Linux processes",
  async () => {
    const { stdout, stderr } = await promisify(execFile)(
      "python3",
      ["-IB", fixture, "--receipts"],
      { timeout: 4500 },
    );
    for (const line of stdout.trim().split("\n")) receipt({ pythonFixture: JSON.parse(line) });
    assert.match(stderr, /\nOK\s*$/);
  },
);

test("sampler selection fixture has the same group/ancestry closure as the teardown selector", () => {
  const root = { pid: 12, pgid: 12 };
  const table = [
    [30, 30, 20],
    [40, 40, 90],
    [20, 20, 12],
    [50, 12, 1],
    [12, 12, 1],
  ].map(([pid, pgid, ppid]) => ({ pid, pgid, ppid }));
  assert.deepEqual(
    ownedProcesses(table, root)
      .map((p) => p.pid)
      .sort((a, b) => a - b),
    [12, 20, 30, 50],
  );
});

async function withServer(run) {
  const child = spawn("python3", ["-IB", fixture, "fixture", "ordinary"], {
    detached: true,
    stdio: ["pipe", "pipe", "pipe"],
  });
  const closed = once(child, "close");
  let pid;
  try {
    const [line] = await once(child.stdout, "data");
    pid = Number(line.toString().trim());
    const native = linuxBirth(child.pid);
    const registered = [];
    const handle = {
      native,
      known: new Map([[native.pid, native]]),
      onNewOwned: (p) => registered.push(p),
    };
    return await run(
      { handle, isAlive: () => ({ alive: child.exitCode === null }) },
      pid,
      registered,
    );
  } finally {
    child.stdin.end();
    const [code, signal] = await closed;
    assert.equal(signal, null);
    assert.equal(code, 0);
    assert.equal(existsSync(`/proc/${child.pid}`), false);
    assert.equal(existsSync(`/proc/${pid}`), false);
    receipt({
      root: child.pid,
      descendant: pid,
      bothAbsent: true,
      code,
      signal,
      wallLimitMs: 8000,
      addressSpaceLimitMiB: 128,
    });
  }
}

linux(
  "real helper hands off separate-group descendants and includes both live RSS observations",
  () =>
    withServer(async (server, pid, registered) => {
      const sample = await fixtureSample(
        "live",
        server,
        readlinkSync(`/proc/${server.handle.native.pid}/exe`),
      );
      assert.equal(sample.samplerVersion, "linux-pidfd-rss.v1");
      const { parent, helper, clock } = sample.timing;
      assert.equal(sample.timing.version, "linux-rss-timing.v1");
      assert.equal(clock.qualified, true, clock.reason);
      assert.equal(BigInt(parent.deadlineNs) - BigInt(parent.entryNs), 750_000_000n);
      assert.equal(helper.observations.length, 2);
      for (const pair of [
        [parent.spawnCallNs, parent.spawnReturnNs],
        [parent.exitNs, parent.closeNs],
        [helper.finalRootStartNs, helper.finalRootEndNs],
        [helper.cleanupStartNs, helper.cleanupEndNs],
        [helper.terminalWrittenNs, helper.terminalFlushedNs],
      ]) {
        assert.ok(pair.every((n) => typeof n === "string"));
        assert.ok(BigInt(pair[0]) <= BigInt(pair[1]));
      }
      assert.ok(parent.requestBytes > 0 && parent.stdoutBytes > 0 && parent.parsedRows > 0);
      assert.equal(parent.stderrBytes, 0);
      assert.ok(BigInt(parent.parseAndValidateNs) >= BigInt(parent.journalNs));
      for (const o of helper.observations) {
        assert.ok(BigInt(o.serializeStartNs) >= BigInt(o.metadataEndNs));
        assert.ok(BigInt(o.outputFrameCallbackNs) >= BigInt(o.flushedNs));
      }
      assert.equal(sample.helper.pipesClosed, true);
      assert.equal(sample.helper.closed, true);
      assert.equal(existsSync(`/proc/${sample.helper.pid}`), false);
      assert.equal(sample.processes.length, 2);
      assert.deepEqual(
        registered.map((p) => p.pid),
        [pid],
      );
      assert.equal(server.handle.known.get(pid).pgid, pid);
      assert.equal(
        sample.summedRssKiB,
        sample.processes.reduce((sum, p) => {
          assert.equal(p.disposition, "live");
          assert.equal(p.rssKiB, Math.max(p.statusKiB.VmRSS, p.smapsRollupKiB.Rss));
          return sum + p.rssKiB;
        }, 0),
      );
    }),
);

for (const kind of [
  "crash",
  "truncated",
  "timeout",
  "bad-ancestry",
  "bad-proof",
  "failure",
  "buffered-at-fault",
  "wrong-version",
  "out-of-order",
  "malformed-frame",
  "duplicate-frame",
  "line-cap",
  "total-cap",
]) {
  linux(
    `bounded helper ${kind} preserves exactly the verified ownership prefix and observed RSS`,
    () =>
      withServer(async (server, pid, registered) => {
        const birth = linuxBirth(pid);
        if (kind === "bad-ancestry") birth.ppid = 1;
        const bound = {
          type: "bound",
          birth,
          binding: {
            pidfdFlags: 0,
            fdinfoPid: kind === "bad-proof" ? pid + 1 : pid,
            verifiedNs: "12",
          },
        };
        const native = server.handle.native;
        const rows = [
          {
            type: "bound",
            birth: native,
            binding: { pidfdFlags: 0, fdinfoPid: native.pid, verifiedNs: "11" },
          },
          bound,
          { type: "rss", source: "VmRSS", birth, rssKiB: 4300001, observedNs: "13" },
          { type: "rss", source: "rollup.Rss", birth, rssKiB: 4250000, observedNs: "14" },
        ];
        if (kind === "failure")
          rows.push({
            type: "failure",
            stage: "binding",
            error: "EMFILE",
            errno: 24,
            syscallError: "EMFILE",
            expected: birth,
            observed: null,
            terminalProof: null,
            startedNs: "10",
            endNs: "15",
            readCalls: 1,
            procInputBytes: 32,
            helperMaxRssKiB: 4096,
            helper: { closed: false },
            verifiedDiscoveries: [],
            summedRssKiB: 0,
          });
        const n = String(process.hrtime.bigint());
        const badFrame = {
          type: "timing",
          version: "linux-rss-timing.v1",
          seq: 1,
          stage: "cleanupBegin",
          ns: [n],
        };
        if (kind === "wrong-version") rows.push({ ...badFrame, version: "unknown" });
        if (kind === "out-of-order") rows.push({ ...badFrame, seq: 4 });
        if (kind === "malformed-frame") rows.push({ ...badFrame, ns: ["no"] });
        if (kind === "duplicate-frame") rows.push({ ...badFrame, stage: "request", seq: 0 });
        const partial = kind === "truncated" ? '{"type":"complete"' : "";
        const data = rows.map((r) => JSON.stringify(r) + "\n").join("") + partial;
        const source = `
import json,os,resource,signal,sys,time
signal.alarm(5)
resource.setrlimit(resource.RLIMIT_AS,(96<<20,96<<20))
r=json.load(sys.stdin)
n=str(time.monotonic_ns())
print(json.dumps(dict(type="timing",version="linux-rss-timing.v1",seq=0,stage="request",ns=[n]*4,clock=dict(implementation=time.get_clock_info("monotonic").implementation,bootId=r["root"]["bootId"],timeNamespace=os.readlink("/proc/self/ns/time")))))
pid=os.getpid()
f=open("/proc/self/stat").read().rsplit(")",1)[1].split()
b=dict(pid=pid,ppid=os.getppid(),pgid=os.getpgrp(),state=f[0],comm="python3",startTicks=f[19],bootId=r["root"]["bootId"],birthVersion="linux-proc-v1")
pad=" "*100000 if sys.argv[2]=="total-cap" else ""
print(json.dumps(dict(type="helper",birth=b))+pad)
exe=os.path.realpath(r["executablePath"])
s=os.stat(exe)
print(json.dumps(dict(type="executable",pathInput=r["executablePath"],executable=dict(path=exe,inode=str(s.st_ino),device=str(s.st_dev))))+pad)
sys.stdout.write(sys.argv[1].replace("\\n", pad+"\\n"))
sys.stdout.flush()
if sys.argv[2]=="line-cap": sys.stdout.write("z"*(128*1024))
sys.stdout.flush()
${kind === "timeout" ? "time.sleep(8)" : "sys.exit(1)"}
`;
        const launch = () => {
          const helper = spawn("python3", ["-Ic", source, data, kind]);
          if (kind === "buffered-at-fault") {
            // The child flushed this complete prefix before an operational
            // error arrives; the normal stdout listener must still consume it.
            helper.stdout.once("data", () => helper.stderr.emit("error", new Error("pipe fault")));
          }
          return helper;
        };
        await assert.rejects(
          fixtureSample(kind, server, readlinkSync(`/proc/${server.handle.native.pid}/exe`), {
            launch,
          }),
          (e) => {
            const d = e.details;
            assert.equal(d.helper.closed, true);
            assert.equal(d.helper.pipesClosed, true);
            assert.equal(existsSync(`/proc/${d.helper.pid}`), false);
            const validDiscovery = !["bad-proof", "bad-ancestry"].includes(kind);
            assert.equal(server.handle.known.has(pid), validDiscovery);
            assert.equal(registered.length, Number(validDiscovery));
            assert.equal(d.verifiedDiscoveries.length, Number(validDiscovery));
            assert.equal(d.summedRssKiB, validDiscovery ? 4300001 : null);
            if (kind === "timeout") {
              assert.equal(d.stage, "deadline");
              assert.ok(d.timing.parent.deadlineDispatchNs);
              assert.ok(d.timing.parent.closureDeadlineNs);
              assert.equal(d.timing.helper.cleanupEndNs, null);
            }
            if (
              [
                "wrong-version",
                "out-of-order",
                "malformed-frame",
                "duplicate-frame",
                "line-cap",
                "total-cap",
              ].includes(kind)
            )
              assert.equal(d.stage, "protocol");
            if (kind === "total-cap") assert.match(d.error, /output byte cap/);
            if (kind === "line-cap") assert.match(d.error, /line byte cap/);
            assert.equal(d.timing.clock.preExitToExitDeliveryResidualNs, null);
            if (kind === "failure") assert.equal(d.errno, 24);
            if (kind === "buffered-at-fault") assert.equal(d.stage, "pipe");
            return true;
          },
        );
      }),
  );
}

for (const mode of [
  "slow-exit",
  "held-stdout",
  "held-past-closure",
  "deferred-parent-data",
  "late-close-exit-zero",
  "other-namespace",
  "bad-envelope",
  "semantic-failure",
  "missing-pre-exit",
]) {
  linux(`helper timing records ${mode}`, () =>
    withServer(async (server) => {
      let holder, holderClosed, deferredUntil;
      const holdsWriter = mode.startsWith("held-");
      const block = (ms) => Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms);
      const launch = () => {
        const child = spawn("python3", ["-IB", fixture, "instrumented", mode], {
          stdio: holdsWriter ? ["pipe", "pipe", "pipe", "pipe"] : ["pipe", "pipe", "pipe"],
        });
        if (holdsWriter) {
          holder = spawn("python3", ["-IB", fixture, "hold-stdout"], {
            stdio: ["pipe", "ignore", "inherit", child.stdio[3]],
          });
          child.stdio[3].destroy();
          holderClosed = once(holder, "close");
          if (mode === "held-stdout")
            child.once("exit", () => setTimeout(() => holder.stdin.end("x"), 55));
        }
        if (mode === "deferred-parent-data")
          child.stdout.once("data", () => {
            block(55);
            deferredUntil = process.hrtime.bigint();
          });
        if (mode === "late-close-exit-zero") child.once("close", () => block(755));
        return child;
      };
      try {
        const promise = fixtureSample(
          mode,
          server,
          readlinkSync(`/proc/${server.handle.native.pid}/exe`),
          { launch },
        );
        if (
          [
            "late-close-exit-zero",
            "missing-pre-exit",
            "semantic-failure",
            "held-past-closure",
          ].includes(mode)
        ) {
          await assert.rejects(promise, (e) => {
            const d = e.details;
            assert.equal(d.helper.closed, mode !== "held-past-closure");
            if (mode === "held-past-closure") {
              const p = d.timing.parent;
              assert.equal(d.stage, "deadline");
              assert.equal(d.helper.pipesClosed, false);
              assert.equal(p.exitCode, 0);
              assert.equal(p.closeNs, null);
              assert.equal(p.stdoutEndNs, null);
              assert.ok(p.closureDispatchNs);
              assert.ok(BigInt(p.closureDeadlineNs) - BigInt(p.deadlineDispatchNs) >= 200_000_000n);
            } else if (mode === "late-close-exit-zero") {
              assert.equal(d.stage, "deadline");
              assert.equal(d.helper.code, 0);
              assert.ok(BigInt(d.timing.parent.closeNs) > BigInt(d.timing.parent.deadlineNs));
              assert.equal(d.timing.parent.deadlineDispatchNs, null);
            } else if (mode === "missing-pre-exit") {
              assert.equal(d.stage, "helper-close");
              assert.equal(d.helper.code, 0);
              assert.equal(d.timing.helper.preExitNs, null);
            } else {
              assert.equal(d.stage, "threads");
              assert.equal(d.errno, 13);
              assert.ok(d.summedRssKiB > 0);
              assert.ok(
                BigInt(d.timing.helper.cleanupStartNs) >= BigInt(d.timing.helper.terminalFlushedNs),
              );
              assert.ok(d.timing.helper.preExitNs);
            }
            return true;
          });
          return;
        }
        const {
          timing: { parent, helper, clock },
        } = await promise;
        if (["other-namespace", "bad-envelope"].includes(mode)) {
          assert.equal(clock.qualified, false);
          assert.equal(
            clock.reason,
            mode === "bad-envelope" ? "causal-envelope" : "time-namespace",
          );
          assert.equal(clock.preExitToExitDeliveryResidualNs, null);
          assert.ok(helper.preExitNs && parent.exitNs);
        } else {
          assert.equal(clock.qualified, true, clock.reason);
          if (mode === "slow-exit")
            assert.ok(BigInt(clock.preExitToExitDeliveryResidualNs) >= 50_000_000n);
          if (mode === "held-stdout") {
            assert.ok(BigInt(parent.stdoutEndNs) - BigInt(parent.exitNs) >= 50_000_000n);
            assert.ok(parent.stdoutCloseNs);
            assert.ok(BigInt(parent.stdoutCloseNs) >= BigInt(parent.stdoutEndNs));
            assert.ok(BigInt(parent.closeNs) >= BigInt(parent.stdoutCloseNs));
          }
          if (mode === "deferred-parent-data") {
            assert.ok(BigInt(parent.firstDataNs) >= deferredUntil);
            assert.ok(BigInt(parent.firstDataNs) - BigInt(helper.requestDecodedNs) >= 50_000_000n);
          }
        }
      } finally {
        if (holder) {
          holder.stdin.end();
          assert.deepEqual(await holderClosed, [0, null]);
          assert.equal(existsSync(`/proc/${holder.pid}`), false);
          receipt({ control: mode, holder: holder.pid, absent: true, code: 0, wallLimitMs: 5000 });
        }
      }
    }),
  );
}
