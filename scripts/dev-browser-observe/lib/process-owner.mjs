import { execFile, spawn } from "node:child_process";
import { promisify } from "node:util";
import { delay, isoNow, run, tryRun } from "./util.mjs";

const execFileAsync = promisify(execFile);

export class PidReuseError extends Error {
  constructor(expected, actual) {
    super(`birth identity mismatch for pid ${expected.pid}; refusing to signal`);
    this.code = "PID_REUSE";
    this.expected = expected;
    this.actual = actual;
  }
}

export function parsePsTable(text) {
  const rows = [];
  for (const line of text.split("\n")) {
    // pid ppid pgid lstart(5 tokens) comm...
    const m = line
      .trim()
      .match(/^(\d+)\s+(\d+)\s+(\d+)\s+(\S+\s+\S+\s+\d+\s+\d+:\d+:\d+\s+\d+)\s+(.*)$/);
    if (m)
      rows.push({
        pid: Number(m[1]),
        ppid: Number(m[2]),
        pgid: Number(m[3]),
        lstart: m[4].replace(/\s+/g, " "),
        comm: m[5],
      });
  }
  return rows;
}

export function psTable(psImpl = run) {
  return parsePsTable(
    psImpl("ps", ["-axo", "pid=,ppid=,pgid=,lstart=,comm="], {
      env: { ...process.env, LC_ALL: "C" },
    }),
  );
}

/** Birth identity of one pid, or null when it no longer exists. */
export function birthIdentity(pid, psImpl = run) {
  const env = { ...process.env, LC_ALL: "C" };
  let text = "";
  try {
    text = psImpl("ps", ["-o", "pid=,ppid=,pgid=,lstart=,comm=", "-p", String(pid)], { env });
  } catch {
    return null;
  }
  const row = parsePsTable(text).find((x) => x.pid === pid);
  if (!row) return null;
  let args = "unavailable";
  try {
    args = psImpl("ps", ["-o", "args=", "-p", String(pid)], { env }).trim();
  } catch {
    args = "unavailable";
  }
  return { ...row, args, observedAt: isoNow() };
}

export function sameBirth(expected, actual) {
  return (
    Boolean(actual) &&
    actual.pid === expected.pid &&
    actual.lstart === expected.lstart &&
    actual.comm === expected.comm &&
    actual.pgid === expected.pgid
  );
}

/** Group members plus ppid-descendants of the root, from one ps snapshot. */
export function ownedProcesses(table, root) {
  const owned = new Map();
  for (const r of table) if (r.pgid === root.pgid) owned.set(r.pid, r);
  let grew = true;
  while (grew) {
    grew = false;
    for (const r of table) {
      if (!owned.has(r.pid) && (r.ppid === root.pid || owned.has(r.ppid))) {
        owned.set(r.pid, r);
        grew = true;
      }
    }
  }
  return [...owned.values()];
}

/**
 * Spawn `cmd` detached (own process group). The returned handle records the
 * birth identity and every exit event (exitCode null + signalCode set when signalled).
 */
function sleepSync(ms) {
  Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms);
}

/** Birth identity read right after spawn, retried a bounded number of times. */
export function readBirthWithRetries(
  pid,
  { tries = 5, waitMs = 20, birthImpl = birthIdentity } = {},
) {
  for (let i = 0; i < tries; i += 1) {
    const birth = birthImpl(pid);
    if (birth) return { birth, attempts: i + 1 };
    if (i < tries - 1) sleepSync(waitMs);
  }
  return { birth: null, attempts: tries };
}

export function spawnOwned(
  cmd,
  args,
  { cwd, env, onStdout, onStderr, birthImpl = birthIdentity } = {},
) {
  const spawnWallMs = Date.now();
  const child = spawn(cmd, args, { cwd, env, detached: true, stdio: ["ignore", "pipe", "pipe"] });
  const handle = {
    child,
    launcher: "none",
    native: null,
    exit: null,
    exited: null,
  };
  handle.exited = new Promise((resolveExit) => {
    child.once("exit", (code, signal) => {
      handle.exit = {
        at: isoNow(),
        exitCode: child.exitCode ?? code,
        signalCode: child.signalCode ?? signal,
      };
      resolveExit(handle.exit);
    });
    child.once("error", (error) => {
      handle.spawnError = String(error?.message ?? error);
      if (handle.exit === null && !child.pid) {
        handle.exit = {
          at: isoNow(),
          exitCode: null,
          signalCode: null,
          spawnError: handle.spawnError,
        };
        resolveExit(handle.exit);
      }
    });
  });
  if (onStdout) child.stdout.on("data", onStdout);
  if (onStderr) child.stderr.on("data", onStderr);
  if (child.pid) {
    const { birth, attempts } = readBirthWithRetries(child.pid, { birthImpl });
    handle.native = birth
      ? { ...birth, executable: cmd, argv: args, spawnWallMs, birthAttempts: attempts }
      : {
          pid: child.pid,
          pgid: child.pid,
          executable: cmd,
          argv: args,
          lstart: "unavailable",
          comm: "unavailable",
          birthUnverifiable: true,
          spawnWallMs,
          birthAttempts: attempts,
        };
    handle.known = new Map(birth ? [[birth.pid, birth]] : []);
  }
  return handle;
}

/** Record the owned process set seen now, so later stops can verify survivors by birth. */
function remember(handle, p, source) {
  handle.known ??= new Map();
  if (handle.known.has(p.pid)) return;
  const entry = { ...p, source };
  handle.known.set(p.pid, entry);
  handle.onNewOwned?.(entry);
}

export function snapshotOwned(handle, psImpl, platform = process.platform) {
  if (!handle.native) return [];
  let seen = ownedProcesses(psTable(psImpl), handle.native);
  if (handle.native.birthUnverifiable) {
    const min = spawnSecondMs(handle.native, platform);
    seen = seen.filter(
      (p) => p.pgid === handle.native.pgid && (parseLstart(p.lstart) ?? -1) >= min,
    );
  }
  for (const p of seen) remember(handle, p, "snapshot");
  return seen;
}

/** Members of `pgid` that started at or after `minMs` (used when the root birth is unverifiable). */
export function pgidScopeSince(table, pgid, minMs) {
  return table.filter((p) => p.pgid === pgid && (parseLstart(p.lstart) ?? -1) >= minMs);
}

/** Live owned processes whose birth still matches a known entry (async ps; ps -p exits 1 when some are gone). */
export async function liveKnownAsync(handle, execImpl = execFileAsync) {
  const known = [...(handle.known?.values() ?? [])];
  if (known.length === 0) return [];
  let text = "";
  try {
    text = (
      await execImpl(
        "ps",
        ["-o", "pid=,ppid=,pgid=,lstart=,comm=", "-p", known.map((k) => k.pid).join(",")],
        { env: { ...process.env, LC_ALL: "C" } },
      )
    ).stdout;
  } catch (error) {
    text = error?.stdout ?? "";
  }
  return parsePsTable(String(text)).filter((p) => known.some((k) => sameBirth(k, p)));
}

export function parseLstart(lstart) {
  const t = Date.parse(lstart);
  return Number.isNaN(t) ? null : t;
}

/**
 * After the root died: members of the root's recorded process group that
 * started no earlier than the root. If a live process now owns pid == pgid
 * with a different birth, the group number was reused and nothing is trusted.
 */
export function pgidSurvivors(table, root) {
  const rootStart = parseLstart(root.lstart);
  const leader = table.find((p) => p.pid === root.pgid);
  if (leader && !sameBirth(root, leader)) return { reused: true, found: [] };
  if (rootStart === null) return { reused: false, found: [], reason: "root-lstart-unparseable" };
  const found = table.filter(
    (p) => p.pgid === root.pgid && p.pid !== root.pid && (parseLstart(p.lstart) ?? -1) >= rootStart,
  );
  return { reused: false, found };
}

function knownMatches(handle, table) {
  const known = [...(handle.known?.values() ?? [])];
  return table.filter((p) => known.some((k) => sameBirth(k, p)));
}

export const STOP_SEQUENCE = ["SIGINT", "SIGTERM", "SIGKILL"];

/**
 * Stop an owned group: SIGINT (zfb's graceful ctrl_c path), then SIGTERM after
 * `intWaitMs`, then SIGKILL after `termWaitMs`, then verify absence within
 * `killWaitMs`. While the root's birth identity verifies, each signal goes to
 * the whole group; once the root is gone the pgid number could be reused, so
 * only individually re-verified known descendants are signalled. Each owned
 * process records the last signal sent before it disappeared.
 */
export async function stopOwned(
  handle,
  {
    intWaitMs = 5000,
    termWaitMs = 3000,
    killWaitMs = 3000,
    killImpl = process.kill,
    psImpl,
    birthImpl = birthIdentity,
    platform = process.platform,
  } = {},
) {
  const report = {
    startedAt: isoNow(),
    sequence: STOP_SEQUENCE,
    waits: { intWaitMs, termWaitMs, killWaitMs },
    steps: [],
  };
  const root = handle.native;
  if (!root)
    return {
      ...report,
      absent: true,
      reason: "never-spawned",
      exit: handle.exit,
      remaining: [],
      endings: [],
      finishedAt: isoNow(),
    };
  report.pgidScans = [];
  const unverifiable = root.birthUnverifiable === true;
  const minLstartMs = unverifiable ? spawnSecondMs(root, platform) : null;
  if (unverifiable) {
    report.mode = "unverifiable-birth-pgid-scope";
    report.scope = {
      pgid: root.pgid,
      minLstartMs,
      lstartSlackMs: lstartSlackMs(platform),
      platform,
      note: "root birth was never readable; only group members started at or after the spawn second (less the platform's lstart slack) are signalled, individually",
    };
  }
  const rootNow = handle.exit === null && !unverifiable ? birthImpl(root.pid) : null;
  if (handle.exit === null && rootNow !== null && !sameBirth(root, rootNow))
    throw new PidReuseError(root, rootNow);
  const rootAlive = handle.exit === null && rootNow !== null;
  if (rootAlive) snapshotOwned(handle, psImpl);
  const current = () => {
    const table = psTable(psImpl);
    if (unverifiable) {
      for (const p of pgidScopeSince(table, root.pgid, minLstartMs))
        remember(handle, p, "pgid-scope-unverifiable");
    } else if (handle.exit !== null || !rootAlive) {
      const scan = pgidSurvivors(table, root);
      for (const p of scan.found) remember(handle, p, "pgid-scan");
      if (scan.reused || scan.found.length)
        report.pgidScans.push({
          at: isoNow(),
          reused: scan.reused,
          found: scan.found.map((p) => p.pid),
        });
    }
    return knownMatches(handle, table);
  };
  report.ownedBefore = current();
  report.rootAlive = rootAlive;
  const endings = new Map();
  for (const k of handle.known?.values() ?? []) {
    if (!report.ownedBefore.some((p) => sameBirth(k, p)))
      endings.set(k.pid, { pid: k.pid, comm: k.comm, endedAfter: "before-stop" });
  }
  let lastSignal = "before-stop";
  let left = report.ownedBefore;
  const settle = () => {
    left = current();
    for (const p of handle.known?.values() ?? []) {
      if (!endings.has(p.pid) && !left.some((x) => sameBirth(p, x)))
        endings.set(p.pid, {
          pid: p.pid,
          comm: p.comm,
          endedAfter: lastSignal,
          observedAt: isoNow(),
        });
    }
  };
  const waits = { SIGINT: intWaitMs, SIGTERM: termWaitMs, SIGKILL: killWaitMs };
  for (const signal of STOP_SEQUENCE) {
    settle();
    if (left.length === 0 && (handle.exit !== null || !rootAlive)) break;
    const groupOk = rootAlive && handle.exit === null;
    const targets = groupOk
      ? [{ target: -root.pgid, label: `group:${root.pgid}` }]
      : left.map((p) => ({ target: p.pid, label: p.pid }));
    for (const t of targets) {
      try {
        killImpl(t.target, signal);
        report.steps.push({ at: isoNow(), signal, target: t.label });
      } catch (error) {
        report.steps.push({
          at: isoNow(),
          signal,
          target: t.label,
          error: String(error.code ?? error),
        });
      }
    }
    lastSignal = signal;
    const deadline = Date.now() + waits[signal];
    settle();
    while (Date.now() < deadline && (left.length > 0 || (groupOk && handle.exit === null))) {
      await delay(100);
      settle();
    }
  }
  await Promise.race([handle.exited, delay(1000)]);
  settle();
  if (root && handle.exit && endings.has(root.pid)) endings.get(root.pid).exit = handle.exit;
  report.remaining = left;
  // The root's own exit event is required too: a live root missing from ps is not absent.
  report.absent = left.length === 0 && handle.exit !== null;
  report.endings = [...endings.values()];
  report.known = [...(handle.known?.values() ?? [])];
  report.exit = handle.exit;
  report.finishedAt = isoNow();
  return report;
}

/** Any recorded server identity that is still alive with the same birth. */
/**
 * Registry rows for owned processes: every descendant, and the root itself
 * when its birth was unverifiable at spawn (its real birth is only known now).
 */
export function shouldRegisterOwned(native, p) {
  return Boolean(native) && (p.pid !== native.pid || native.birthUnverifiable === true);
}

/**
 * How much earlier than a process's real start its `ps -o lstart` can read.
 * Linux procps prints btime + start_time/HZ, and /proc/stat's btime is the
 * boot time truncated to whole seconds, so lstart can be up to 1 s early
 * (by the boot time's fraction, constant per boot). macOS reports the real
 * start time, truncated, which is never early.
 */
export function lstartSlackMs(platform = process.platform) {
  return platform === "linux" ? 1000 : 0;
}

/** Earliest lstart (ms) a process started at or after `native`'s spawn can show on this platform. */
export function spawnSecondMs(native, platform = process.platform) {
  return Math.floor(native.spawnWallMs / 1000) * 1000 - lstartSlackMs(platform);
}

/**
 * Registry rows still alive. A verified row matches by birth identity. A row
 * whose birth was unverifiable at spawn matches by pid + pgid with a start at
 * or after its spawn second.
 */
export function liveOwnedFromRegistry(
  entries,
  birthImpl = birthIdentity,
  platform = process.platform,
) {
  return entries.filter((e) => {
    const now = birthImpl(e.pid);
    if (!now) return false;
    if (e.birthUnverifiable)
      return now.pgid === e.pgid && (parseLstart(now.lstart) ?? -1) >= spawnSecondMs(e, platform);
    return sameBirth(e, now);
  });
}
