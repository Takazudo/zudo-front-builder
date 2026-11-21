import { describe, expect, it } from "vitest";
import {
  liveOwnedFromRegistry,
  lstartSlackMs,
  parsePsTable,
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
    expect(spawnSecondMs({ spawnWallMs: SPAWN_WALL_MS }, "darwin")).toBe(
      Date.parse("Sat Nov 21 03:10:31 2026"),
    );
    expect(spawnSecondMs({ spawnWallMs: SPAWN_WALL_MS }, "linux")).toBe(
      Date.parse("Sat Nov 21 03:10:30 2026"),
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
    expect(report.scope).toMatchObject({ lstartSlackMs: 1000, platform: "linux" });
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
});
