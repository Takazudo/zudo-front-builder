import { EventEmitter } from "node:events";
import { createWriteStream, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { ObserverPage } from "../lib/browser.mjs";
import { liveKnownAsync, readBirthWithRetries } from "../lib/process-owner.mjs";
import { LineSplitter, ServerLog, parseReadyUrl, pipesClosed } from "../lib/server.mjs";
import { runBatch } from "../lib/state-machine.mjs";
import { deps, fakeFs, fakePage } from "./fakes.mjs";

const tmps = [];
const tmp = () => {
  const d = mkdtempSync(join(tmpdir(), "dbo-r5-"));
  tmps.push(d);
  return d;
};
afterEach(() => {
  while (tmps.length) rmSync(tmps.pop(), { recursive: true, force: true });
});

describe("#1 server.log never crashes on late child output", () => {
  it("drops and counts writes after end instead of emitting write-after-end", async () => {
    const path = join(tmp(), "server.log");
    const log = new ServerLog(createWriteStream(path, { flags: "a" }));
    log.write("before\n");
    await log.end();
    log.write("after end\n");
    await new Promise((r) => setImmediate(r));
    expect(log.droppedAfterEnd).toBe(1);
    expect(log.errors).toEqual([]);
    expect(readFileSync(path, "utf8")).toBe("before\n");
  });

  it("records stream errors instead of throwing", async () => {
    const log = new ServerLog(createWriteStream(join(tmp(), "missing-dir", "server.log")));
    log.write("x\n");
    await new Promise((r) => setTimeout(r, 50));
    expect(log.errors[0]).toMatch(/ENOENT/);
  });

  it("pipesClosed is bounded when a pipe never closes", async () => {
    const never = Object.assign(new EventEmitter(), { destroyed: false, closed: false });
    expect(await pipesClosed({ stdout: never, stderr: never }, 20)).toBe("timeout");
    const closed = { destroyed: true };
    expect(await pipesClosed({ stdout: closed, stderr: closed }, 20)).toBe("closed");
  });
});

describe("#4 ready is parsed from complete lines only", () => {
  it("does not match a URL truncated at a chunk boundary", () => {
    expect(parseReadyUrl("→ ready on http://127.0.0.1:12")).toBeNull();
    expect(parseReadyUrl("→ ready on http://127.0.0.1:1234\n")).toBe("http://127.0.0.1:1234");
    expect(parseReadyUrl("→ ready on http://127.0.0.1:1234/\r\n")).toBe("http://127.0.0.1:1234");
  });

  it("LineSplitter holds the tail until the terminator arrives", () => {
    const s = new LineSplitter();
    expect(s.push("info listening\n→ ready on http://127.0.0.1:12")).toEqual(["info listening"]);
    expect(s.push("34\r\nnext")).toEqual(["→ ready on http://127.0.0.1:1234"]);
    expect(s.flush()).toEqual(["next"]);
  });
});

describe("#5 birth read retries", () => {
  it("retries a bounded number of times", () => {
    let n = 0;
    const birthImpl = () => (++n < 3 ? null : { pid: 1, lstart: "x" });
    expect(readBirthWithRetries(1, { birthImpl, waitMs: 1 })).toMatchObject({
      attempts: 3,
      birth: { pid: 1 },
    });
    expect(readBirthWithRetries(1, { birthImpl: () => null, tries: 2, waitMs: 1 })).toEqual({
      birth: null,
      attempts: 2,
    });
  });
});

describe("#6 armed-wait sampling only samples birth-matching live pids", () => {
  it("drops a known pid whose current ps row has a different birth", async () => {
    const handle = {
      known: new Map([
        [10, { pid: 10, pgid: 10, lstart: "Sat Nov 21 06:00:00 2026", comm: "zfb" }],
        [11, { pid: 11, pgid: 10, lstart: "Sat Nov 21 06:00:01 2026", comm: "helper" }],
        [12, { pid: 12, pgid: 10, lstart: "Sat Nov 21 06:00:02 2026", comm: "gone" }],
      ]),
    };
    const execImpl = async () => {
      const err = new Error("ps exited 1");
      err.stdout =
        "   10     1    10 Sat Nov 21 06:00:00 2026     zfb\n   11     1    77 Sat Nov 21 07:30:00 2026     reused\n";
      throw err;
    };
    expect((await liveKnownAsync(handle, execImpl)).map((p) => p.pid)).toEqual([10]);
  });
});

describe("#7 calibration ping is bounded", () => {
  it("times out, deletes its waiter and reports a failure instead of hanging", async () => {
    const p = new ObserverPage(null, "", () => 1n);
    p.page = { evaluate: async () => true };
    const r = await p.ping("direct", 30);
    expect(r).toMatchObject({ ok: false, timedOut: true, ms: null });
    expect(p.pingWaiters.size).toBe(0);
  });
});

describe("#9 after-stop cleanup never applies the restore-blocked chmod", () => {
  it("restores the file and records the skipped block", async () => {
    const scen = [
      {
        id: "neg",
        set: "negative-controls",
        ops: [
          {
            id: "blk",
            op: "edit-title",
            article: "a",
            writeMethod: "in-place",
            inject: "restore-blocked",
          },
          { id: "late", op: "edit-body", article: "a", writeMethod: "in-place" },
        ],
      },
    ];
    const fs = fakeFs();
    const blockCalls = [];
    const block = fs.blockWrite;
    fs.blockWrite = (p) => {
      blockCalls.push(p);
      return block(p);
    };
    const d = deps({
      page: fakePage({ behaviors: [{ kind: "notify", t1: 2_000_000_000n }, { kind: "timeout" }] }),
      fs,
    });
    const { accounting } = await runBatch({ scenarios: scen, deps: d });
    expect(blockCalls).toEqual([]);
    expect(fs.files["a.md"]).toBe("v0");
    expect(accounting).toMatchObject({ stopReason: "timeout", restorationFailures: 0 });
  });
});
