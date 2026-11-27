import { execFile, spawn } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { promisify } from "node:util";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { acquireLock, LockBusyError } from "../lib/lock.mjs";
import { HARNESS_DIR } from "../lib/util.mjs";

let dir, path;
beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), "lock-helper-"));
  path = join(dir, "lock");
});
afterEach(() => rmSync(dir, { recursive: true, force: true }));

function helper(body) {
  const perl = join(dir, "perl");
  writeFileSync(perl, "#!/usr/bin/perl\nuse strict; $|=1;\n" + body + "\n", { mode: 0o700 });
  return perl;
}
async function refusal(promise) {
  try {
    await promise;
  } catch (error) {
    return error;
  }
  throw Error("expected refusal");
}
function expectClosed(error) {
  const h = error.lockHelper;
  expect(h.pid).toBeGreaterThan(0);
  expect(h.cleanupVerified).toBe(true);
  expect(h.close).toEqual(h.exit);
  expect(h.pipesClosed).toEqual([true, true, true]);
  if (process.platform === "linux") {
    expect(h.birth.pid).toBe(h.pid);
    expect(existsSync("/proc/" + h.pid)).toBe(false);
  }
}

it("waits for the complete own-PID frame, including a split newline", async () => {
  const perl = helper(
    'print "LOCKED"; select undef,undef,undef,0.05; print " $$"; select undef,undef,undef,0.05; open(my $fh,">",$ARGV[-1].".ready") or die; close $fh; print "\\n"; while (<STDIN>) {}',
  );
  const lock = await acquireLock(path, { perl });
  try {
    expect(existsSync(path + ".ready")).toBe(true);
  } finally {
    expect(await lock.release()).toEqual({ code: 0, signal: null });
  }
});

it.each(["LOCKED 0\\n", "LOCKEDX\\n", "BUSY!\\n", "BUSY\\nextra"])(
  "rejects and reaps malformed frame %s",
  async (frame) => {
    const error = await refusal(
      acquireLock(path, { perl: helper('print "' + frame + '"; while (<STDIN>) {}') }),
    );
    expect(error.message).toContain("invalid lock helper response");
    expectClosed(error);
  },
);

it("preserves a real timeout, incomplete frame and bounded termination evidence", async () => {
  const error = await refusal(
    acquireLock(path, { perl: helper('print "LOCKED"; select undef,undef,undef,15;') }),
  );
  expect(error.message).toBe("lock helper acquisition timed out");
  expectClosed(error);
  const h = error.lockHelper;
  expect(BigInt(h.deadlineNs) - BigInt(h.startNs)).toBe(3000000000n);
  expect(h.stdout).toBe("LOCKED");
  expect(h.signalRequest).toEqual({ signal: "SIGKILL", sent: true });
  expect(h.exit.signal).toBe("SIGKILL");
}, 10000);

it("distinguishes genuine contention from a bad helper exit", async () => {
  const lock = await acquireLock(path);
  try {
    const busy = await refusal(acquireLock(path));
    expect(busy).toBeInstanceOf(LockBusyError);
    expectClosed(busy);
    expect(busy.lockHelper.exit).toEqual({ code: 3, signal: null });
  } finally {
    await lock.release();
  }
  const bad = await refusal(acquireLock(path, { perl: helper('print "BUSY\\n";exit 4;') }));
  expect(bad).not.toBeInstanceOf(LockBusyError);
  expectClosed(bad);
  expect(bad.lockHelper.exit.code).toBe(4);
});

it("retains cancellation ownership until the original helper closes", async () => {
  const controller = new AbortController();
  const pending = acquireLock(path, {
    perl: helper("select undef,undef,undef,15;"),
    signal: controller.signal,
  });
  controller.abort(new Error("controlled cancel"));
  const error = await refusal(pending);
  expect(error.message).toBe("lock helper acquisition cancelled");
  expectClosed(error);
});

it("journals a prelaunch CLI refusal without consuming a workload label", async () => {
  helper('print "LOCKED 0\\n";while (<STDIN>) {}');
  const profile = JSON.parse(
    readFileSync(join(HARNESS_DIR, "profiles", "tiny-internal-a0.example.json")),
  );
  profile.outDir = join(dir, "out");
  profile.lockPath = path;
  writeFileSync(join(dir, "profile.json"), JSON.stringify(profile));
  const run = () =>
    promisify(execFile)(
      process.execPath,
      [
        join(HARNESS_DIR, "cli.mjs"),
        "run",
        "--profile",
        join(dir, "profile.json"),
        "--label",
        "first",
      ],
      {
        env: {
          ...process.env,
          PATH: dir + ":" + process.env.PATH,
          XDG_STATE_HOME: join(dir, "state"),
        },
        timeout: 10000,
      },
    );
  await expect(run()).rejects.toMatchObject({ code: 8 });
  await expect(run()).rejects.toMatchObject({ code: 8 });
  const journal = readFileSync(
    join(profile.outDir, ".dev-browser-observe-lock-failures.jsonl"),
    "utf8",
  )
    .trim()
    .split("\n")
    .map(JSON.parse);
  expect(journal).toHaveLength(2);
  expect(journal[0].supervisorId).not.toBe(journal[1].supervisorId);
  for (const entry of journal) {
    expect(entry.label).toBe("first");
    expectClosed(entry);
  }
  expect(existsSync(join(profile.outDir, "first"))).toBe(false);
}, 25000);

it("re-arms an early acquisition alarm until the monotonic deadline", async () => {
  const original = globalThis.setTimeout;
  const timers = vi
    .spyOn(globalThis, "setTimeout")
    .mockImplementationOnce((callback) => original(callback, 1));
  try {
    const perl = helper('select undef,undef,undef,0.05; print "LOCKED $$\\n"; while (<STDIN>) {}');
    const lock = await acquireLock(path, { perl });
    try {
      expect(timers.mock.calls.length).toBeGreaterThanOrEqual(2);
    } finally {
      await lock.release();
    }
  } finally {
    timers.mockRestore();
  }
});

it("reports unresolved cleanup when the original child close milestone is absent", async () => {
  let deliverClose;
  const spawnHelper = (...args) => {
    const child = spawn(...args);
    // Delay only the delivery of the real close event. The owned process
    // actually exits and leaves no descendant or open descriptor behind.
    const once = child.once.bind(child);
    child.once = (event, listener) =>
      event === "close"
        ? once(event, (...values) => {
            deliverClose = () => listener(...values);
          })
        : once(event, listener);
    return child;
  };
  const error = await refusal(
    acquireLock(path, {
      perl: helper('print "BROKEN\\n"; select undef,undef,undef,15;'),
      spawnHelper,
    }),
  );
  expect(error.message).toContain("invalid lock helper response");
  expect(error.lockHelper.exit.signal).toBe("SIGKILL");
  expect(error.lockHelper.close).toBeNull();
  expect(error.lockHelper.cleanupVerified).toBe(false);
  expect(error.lockHelper.localHandlesDetached).toBe(true);
  deliverClose();
  expect(error.lockHelper.close).toBeNull();
}, 6000);

it("rejects the exact supplied LOCKED frame when the child exit is already known", async () => {
  const spawnHelper = (...args) => {
    const child = spawn(...args);
    child.stdout.pause();
    child.once("exit", () => child.stdout.resume());
    // Preserve the exit-before-frame ordering even when a data listener is added.
    const on = child.stdout.on.bind(child.stdout);
    child.stdout.on = (event, listener) => {
      if (event !== "data") return on(event, listener);
      return on(event, (chunk) => {
        if (child.exitCode !== null) listener(chunk);
        else child.once("exit", () => listener(chunk));
      });
    };
    return child;
  };
  const error = await refusal(
    acquireLock(path, { perl: helper('print "LOCKED $$\\n";exit 0;'), spawnHelper }),
  );
  expect(error.message).toBe("lock helper exited before acquisition decision");
  expectClosed(error);
});
