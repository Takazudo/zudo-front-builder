import { describe, expect, it } from "vitest";
import {
  lstartSlackMs,
  parsePsTable,
  PidReuseError,
  sameBirth,
  scopeFloorMs,
  spawnSecondMs,
  stopOwned,
} from "../lib/process-owner.mjs";

// procps (Linux) `ps -axo pid=,ppid=,pgid=,lstart=,comm=` rows. procps prints lstart as
// btime + start_time/HZ with btime truncated to whole seconds, so a process spawned at
// 03:10:31.4 can show 03:10:30 when the boot time's fraction exceeds 0.4.
const SPAWN_WALL_MS = Date.parse("Sat Nov 21 03:10:31 2026") + 400;
const PROCPS_ROW = "    500       1     500 Sat Nov 21 03:10:30 2026 node";

function unverifiableHandle() {
  let resolveExit;
  const handle = {
    native: {
      pid: 500,
      pgid: 500,
      lstart: "unavailable",
      comm: "unavailable",
      birthUnverifiable: true,
      spawnWallMs: SPAWN_WALL_MS,
    },
    exit: null,
    known: new Map(),
  };
  handle.exited = new Promise((r) => {
    resolveExit = r;
  });
  return { handle, resolveExit };
}

describe("Linux procps lstart can read up to 1 s before the real start", () => {
  it("the procps row parses, and the slack is Linux-only", () => {
    expect(parsePsTable(PROCPS_ROW)).toEqual([
      { pid: 500, ppid: 1, pgid: 500, lstart: "Sat Nov 21 03:10:30 2026", comm: "node" },
    ]);
    expect(lstartSlackMs("linux")).toBe(1000);
    expect(lstartSlackMs("darwin")).toBe(0);
    const root = { pid: 500, pgid: 500, spawnWallMs: SPAWN_WALL_MS };
    expect(spawnSecondMs(root)).toBe(Date.parse("Sat Nov 21 03:10:31 2026"));
    expect(scopeFloorMs(root, root, "darwin")).toBe(Date.parse("Sat Nov 21 03:10:31 2026"));
    expect(scopeFloorMs(root, root, "linux")).toBe(Date.parse("Sat Nov 21 03:10:30 2026"));
    expect(scopeFloorMs(root, { pid: 501, pgid: 500 }, "linux")).toBe(
      Date.parse("Sat Nov 21 03:10:31 2026"),
    );
  });

  it("Linux legacy rows cannot authorize a signal even with matching ps display fields", async () => {
    const { handle } = unverifiableHandle();
    const actual = parsePsTable(PROCPS_ROW)[0];
    const killImpl = () => {
      throw new Error("unsafe signal");
    };
    await expect(
      stopOwned(handle, { platform: "linux", birthImpl: () => actual, killImpl }),
    ).rejects.toMatchObject({ code: "PID_REUSE", actual, steps: [] });
  });
});

describe("birth identity does not depend on comm (Linux Node renames its main thread)", () => {
  // procps rows for one process: `comm` reads "node" right after spawn and "MainThread"
  // once Node calls prctl(PR_SET_NAME); pid, pgid, lstart and args do not change.
  const ARGS = "/usr/bin/node -e setInterval(() => {}, 1000)";
  const recorded = {
    ...parsePsTable("    700       1     700 Sat Nov 21 03:10:31 2026 node")[0],
    args: ARGS,
  };
  const observed = (row, args = ARGS) => ({ ...parsePsTable(row)[0], args });
  const MAIN_THREAD = "    700       1     700 Sat Nov 21 03:10:31 2026 MainThread";

  it("a MainThread observation against a recorded node is the same birth", () => {
    expect(observed(MAIN_THREAD).comm).toBe("MainThread");
    expect(sameBirth(recorded, observed(MAIN_THREAD))).toBe(true);
    expect(sameBirth(recorded, { ...observed(MAIN_THREAD), args: "unavailable" })).toBe(true);
    const tableRow = parsePsTable(MAIN_THREAD)[0];
    expect(tableRow.args).toBeUndefined();
    expect(sameBirth(recorded, tableRow)).toBe(true);
  });

  it("a differing lstart, pgid or args is still a different birth", () => {
    expect(
      sameBirth(recorded, observed("    700       1     700 Sat Nov 21 03:10:32 2026 MainThread")),
    ).toBe(false);
    expect(
      sameBirth(recorded, observed("    700       1     701 Sat Nov 21 03:10:31 2026 node")),
    ).toBe(false);
    expect(sameBirth(recorded, observed(MAIN_THREAD, "/usr/bin/python3 other.py"))).toBe(false);
    expect(
      sameBirth(
        recorded,
        observed("    700       1     700 Sat Nov 21 03:10:31 2026 node", "/usr/bin/node other.js"),
      ),
    ).toBe(false);
    expect(sameBirth(recorded, null)).toBe(false);
  });

  it("stop signals a root whose comm became MainThread, and refuses a reused pid", async () => {
    const run = async (now) => {
      let resolveExit;
      const handle = { native: { ...recorded, spawnWallMs: 0 }, exit: null, known: new Map() };
      handle.exited = new Promise((r) => {
        resolveExit = r;
      });
      let alive = true;
      const signals = [];
      const report = stopOwned(handle, {
        intWaitMs: 200,
        termWaitMs: 200,
        killWaitMs: 200,
        platform: "darwin",
        birthImpl: () => (alive ? now : null),
        psImpl: () => (alive ? `${MAIN_THREAD}\n` : ""),
        killImpl: (pid, signal) => {
          signals.push([pid, signal]);
          alive = false;
          handle.exit = { exitCode: null, signalCode: signal };
          resolveExit(handle.exit);
        },
      });
      return { report, signals };
    };
    const same = await run(observed(MAIN_THREAD));
    expect((await same.report).absent).toBe(true);
    expect(same.signals[0]).toEqual([-700, "SIGINT"]);
    const reused = await run(
      observed("    700       1     700 Sat Nov 21 04:00:00 2026 MainThread"),
    );
    await expect(reused.report).rejects.toBeInstanceOf(PidReuseError);
    expect(reused.signals).toEqual([]);
  });
});
