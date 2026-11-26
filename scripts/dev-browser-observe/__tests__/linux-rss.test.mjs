import assert from "node:assert/strict";
import { execFile, spawn } from "node:child_process";
import { once } from "node:events";
import { existsSync, readlinkSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";
import { test } from "vitest";
import { linuxBirth } from "../lib/linux-birth.mjs";
import { sampleRendererMemory } from "../lib/linux-rss-sample.mjs";
import { ownedProcesses } from "../lib/process-owner.mjs";

const linux = test.skipIf(process.platform !== "linux");
const fixture = fileURLToPath(new URL("./linux-rss-fixture.py", import.meta.url));

linux(
  "group pidfd, proc-dir races, partial RSS, identity and cleanup on controlled Linux processes",
  async () => {
    const { stderr } = await promisify(execFile)("python3", ["-IB", fixture], { timeout: 4500 });
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
  try {
    const [line] = await once(child.stdout, "data");
    const pid = Number(line.toString().trim());
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
  }
}

linux(
  "real helper hands off separate-group descendants and includes both live RSS observations",
  () =>
    withServer(async (server, pid, registered) => {
      const sample = await sampleRendererMemory(
        server,
        readlinkSync(`/proc/${server.handle.native.pid}/exe`),
      );
      assert.equal(sample.samplerVersion, "linux-pidfd-rss.v1");
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
        const partial = kind === "truncated" ? '{"type":"complete"' : "";
        const data = rows.map((r) => JSON.stringify(r) + "\n").join("") + partial;
        const source = `
import json,os,sys,time
r=json.load(sys.stdin)
pid=os.getpid()
f=open("/proc/self/stat").read().rsplit(")",1)[1].split()
b=dict(pid=pid,ppid=os.getppid(),pgid=os.getpgrp(),state=f[0],comm="python3",startTicks=f[19],bootId=r["root"]["bootId"],birthVersion="linux-proc-v1")
print(json.dumps(dict(type="helper",birth=b)))
exe=os.path.realpath(r["executablePath"])
s=os.stat(exe)
print(json.dumps(dict(type="executable",pathInput=r["executablePath"],executable=dict(path=exe,inode=str(s.st_ino),device=str(s.st_dev)))))
sys.stdout.write(sys.argv[1])
sys.stdout.flush()
${kind === "timeout" ? "time.sleep(8)" : "sys.exit(1)"}
`;
        const launch = () => {
          const helper = spawn("python3", ["-Ic", source, data]);
          if (kind === "buffered-at-fault") {
            // The child flushed this complete prefix before an operational
            // error arrives; the normal stdout listener must still consume it.
            helper.stdout.once("data", () => helper.stderr.emit("error", new Error("pipe fault")));
          }
          return helper;
        };
        await assert.rejects(
          sampleRendererMemory(server, readlinkSync(`/proc/${server.handle.native.pid}/exe`), {
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
            if (kind === "timeout") assert.equal(d.stage, "deadline");
            if (kind === "failure") assert.equal(d.errno, 24);
            if (kind === "buffered-at-fault") assert.equal(d.stage, "pipe");
            return true;
          },
        );
      }),
  );
}
