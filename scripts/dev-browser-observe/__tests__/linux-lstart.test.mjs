import { describe, expect, it } from "vitest";
import {
  liveOwnedFromRegistry,
  lstartSlackMs,
  parsePsTable,
  PidReuseError,
  pgidScopeSince,
  sameBirth,
  scopeFloorMs,
  snapshotOwned,
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

  it("the unverifiable-birth stop finds and signals the root whose procps lstart is the previous second (CI health failure)", async () => {
    const { handle, resolveExit } = unverifiableHandle();
    let alive = true;
    const psImpl = () => (alive ? `${PROCPS_ROW}\n` : "");
    const signals = [];
    const killImpl = (pid, signal) => {
      signals.push([pid, signal]);
      if (signal === "SIGINT") {
        alive = false;
        handle.exit = { exitCode: null, signalCode: "SIGINT" };
        resolveExit(handle.exit);
      }
    };
    const report = await stopOwned(handle, {
      intWaitMs: 200,
      termWaitMs: 200,
      killWaitMs: 200,
      killImpl,
      psImpl,
      platform: "linux",
    });
    expect(report.mode).toBe("unverifiable-birth-pgid-scope");
    expect(report.scope).toMatchObject({
      lstartSlackMs: 1000,
      platform: "linux",
      minLstartMs: Date.parse("Sat Nov 21 03:10:31 2026"),
      rootMinLstartMs: Date.parse("Sat Nov 21 03:10:30 2026"),
    });
    expect(signals).toEqual([[500, "SIGINT"]]);
    expect(report.endings.find((e) => e.pid === 500)).toMatchObject({ endedAfter: "SIGINT" });
    expect(report.absent).toBe(true);
  });

  it("the snapshot registers that root and the next-batch guard still recognises it", () => {
    const { handle } = unverifiableHandle();
    const seen = snapshotOwned(handle, () => `${PROCPS_ROW}\n`, "linux");
    expect(seen.map((p) => p.pid)).toEqual([500]);
    const row = { type: "spawn", ...handle.native };
    const birth = () => parsePsTable(PROCPS_ROW)[0];
    expect(liveOwnedFromRegistry([row], birth, "linux")).toHaveLength(1);
    expect(liveOwnedFromRegistry([row], birth, "darwin")).toHaveLength(0);
  });

  describe("the slack applies to the root pid only", () => {
    // 501 is a group member whose lstart sits inside the root's slack window: on a reused
    // pgid it could be an older stray process, so it must stay out of the scope.
    const STRAY_ROW = "    501     500     500 Sat Nov 21 03:10:30 2026 node";
    const LATER_ROW = "    502     500     500 Sat Nov 21 03:10:31 2026 node";
    const table = `${PROCPS_ROW}\n${STRAY_ROW}\n${LATER_ROW}\n`;

    it("pgid scope and snapshot keep the root and later members, never the in-slack non-root", () => {
      const { handle } = unverifiableHandle();
      const scope = pgidScopeSince(parsePsTable(table), handle.native, "linux");
      expect(scope.map((p) => p.pid)).toEqual([500, 502]);
      expect(snapshotOwned(handle, () => table, "linux").map((p) => p.pid)).toEqual([500, 502]);
      expect(
        pgidScopeSince(parsePsTable(table), handle.native, "darwin").map((p) => p.pid),
      ).toEqual([502]);
    });

    it("stop on linux signals the root and the later member, never the in-slack non-root", async () => {
      const { handle, resolveExit } = unverifiableHandle();
      const alive = new Set([500, 501, 502]);
      const rows = { 500: PROCPS_ROW, 501: STRAY_ROW, 502: LATER_ROW };
      const psImpl = () => [...alive].map((pid) => rows[pid]).join("\n");
      const signals = [];
      const killImpl = (pid, signal) => {
        signals.push([pid, signal]);
        if (signal === "SIGINT") {
          alive.delete(pid);
          if (pid === 500) {
            handle.exit = { exitCode: null, signalCode: "SIGINT" };
            resolveExit(handle.exit);
          }
        }
      };
      const report = await stopOwned(handle, {
        intWaitMs: 200,
        termWaitMs: 200,
        killWaitMs: 200,
        killImpl,
        psImpl,
        platform: "linux",
      });
      expect(signals.map(([pid]) => pid).sort()).toEqual([500, 502]);
      expect(signals.some(([pid]) => pid === 501)).toBe(false);
      expect(report.ownedBefore.map((p) => p.pid).sort()).toEqual([500, 502]);
    });

    it("the registry guard allows the slack for a root row only", () => {
      const birthOf = (row) => () => parsePsTable(row)[0];
      const rootRow = {
        type: "spawn",
        pid: 500,
        pgid: 500,
        birthUnverifiable: true,
        spawnWallMs: SPAWN_WALL_MS,
      };
      const memberRow = { ...rootRow, type: "owned", pid: 501 };
      expect(liveOwnedFromRegistry([rootRow], birthOf(PROCPS_ROW), "linux")).toHaveLength(1);
      expect(liveOwnedFromRegistry([memberRow], birthOf(STRAY_ROW), "linux")).toHaveLength(0);
    });
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
        platform: "linux",
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
