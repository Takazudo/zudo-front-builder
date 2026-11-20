import { spawn } from "node:child_process";
import { delay, isoNow, run, tryRun } from "./util.mjs";

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
export function spawnOwned(cmd, args, { cwd, env, onStdout, onStderr } = {}) {
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
    const birth = birthIdentity(child.pid);
    handle.native = birth
      ? { ...birth, executable: cmd, argv: args }
      : {
          pid: child.pid,
          pgid: child.pid,
          executable: cmd,
          argv: args,
          lstart: "unavailable",
          comm: "unavailable",
        };
    handle.known = new Map(birth ? [[birth.pid, birth]] : []);
  }
  return handle;
}

/** Record the owned process set seen now, so later stops can verify survivors by birth. */
export function snapshotOwned(handle, psImpl) {
  if (!handle.native) return [];
  const seen = ownedProcesses(psTable(psImpl), handle.native);
  handle.known ??= new Map();
  for (const p of seen) if (!handle.known.has(p.pid)) handle.known.set(p.pid, p);
  return seen;
}

function knownMatches(handle, table) {
  const known = [...(handle.known?.values() ?? [])];
  return table.filter((p) => known.some((k) => sameBirth(k, p)));
}

/**
 * Stop an owned group. While the root's birth identity still verifies, the
 * whole group gets SIGTERM; once the root is gone the pgid number could be
 * reused, so only individually re-verified known descendants are signalled.
 * After a bounded wait, verified survivors get SIGKILL and absence is checked.
 */
export async function stopOwned(
  handle,
  {
    termWaitMs = 5000,
    killWaitMs = 3000,
    killImpl = process.kill,
    psImpl,
    birthImpl = birthIdentity,
  } = {},
) {
  const report = { startedAt: isoNow(), steps: [] };
  const root = handle.native;
  if (!root)
    return {
      ...report,
      absent: true,
      reason: "never-spawned",
      exit: handle.exit,
      remaining: [],
      finishedAt: isoNow(),
    };
  const rootNow = handle.exit === null ? birthImpl(root.pid) : null;
  if (handle.exit === null && rootNow !== null && !sameBirth(root, rootNow))
    throw new PidReuseError(root, rootNow);
  const rootAlive = handle.exit === null && rootNow !== null;
  if (rootAlive) snapshotOwned(handle, psImpl);
  const current = () => knownMatches(handle, psTable(psImpl));
  report.ownedBefore = current();
  report.rootAlive = rootAlive;
  if (rootAlive) {
    try {
      killImpl(-root.pgid, "SIGTERM");
      report.steps.push({ at: isoNow(), signal: "SIGTERM", target: `group:${root.pgid}` });
    } catch (error) {
      report.steps.push({
        at: isoNow(),
        signal: "SIGTERM",
        target: `group:${root.pgid}`,
        error: String(error.code ?? error),
      });
    }
  } else {
    for (const p of report.ownedBefore) {
      try {
        killImpl(p.pid, "SIGTERM");
        report.steps.push({ at: isoNow(), signal: "SIGTERM", target: p.pid });
      } catch (error) {
        report.steps.push({
          at: isoNow(),
          signal: "SIGTERM",
          target: p.pid,
          error: String(error.code ?? error),
        });
      }
    }
  }
  const deadline = Date.now() + termWaitMs;
  let left = current();
  while (Date.now() < deadline && (left.length > 0 || (rootAlive && handle.exit === null))) {
    await delay(100);
    left = current();
  }
  for (const s of left) {
    try {
      killImpl(s.pid, "SIGKILL");
      report.steps.push({ at: isoNow(), signal: "SIGKILL", target: s.pid });
    } catch (error) {
      report.steps.push({
        at: isoNow(),
        signal: "SIGKILL",
        target: s.pid,
        error: String(error.code ?? error),
      });
    }
  }
  const killDeadline = Date.now() + killWaitMs;
  while (left.length > 0 && Date.now() < killDeadline) {
    await delay(100);
    left = current();
  }
  await Promise.race([handle.exited, delay(1000)]);
  report.remaining = left;
  report.absent = left.length === 0;
  report.exit = handle.exit;
  report.finishedAt = isoNow();
  return report;
}

/** Any recorded server identity that is still alive with the same birth. */
export function liveOwnedFromRegistry(entries, birthImpl = birthIdentity) {
  return entries.filter((e) => sameBirth(e, birthImpl(e.pid)));
}
