import { execFile, spawn } from "node:child_process";
import { linuxBirth, LINUX_BIRTH, sameLinuxBirth } from "./linux-birth.mjs";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";
import { delay, isoNow, run } from "./util.mjs";

const execFileAsync = promisify(execFile);

export class PidReuseError extends Error {
  constructor(expected, actual) {
    super(`birth identity mismatch for pid ${expected.pid}; refusing to signal`);
    this.code = "PID_REUSE";
    this.expected = expected;
    this.actual = actual;
    this.differingFields = [
      "pid",
      "pgid",
      "birthVersion",
      "bootId",
      "startTicks",
      "birthUnverifiable",
    ].filter((k) => expected?.[k] !== actual?.[k]);
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
      timeout: 1500,
      killSignal: "SIGKILL",
    }),
  );
}

/** Birth identity of one pid, or null when it no longer exists. */
export function birthIdentity(pid, psImpl = run) {
  const env = { ...process.env, LC_ALL: "C" };
  const kernel = process.platform === "linux" && psImpl === run;
  const first = kernel ? linuxBirth(pid) : null;
  if (kernel && (!first || first.birthUnverifiable)) return first;
  let text = "";
  try {
    text = psImpl("ps", ["-o", "pid=,ppid=,pgid=,lstart=,comm=", "-p", String(pid)], {
      env,
      timeout: 1500,
      killSignal: "SIGKILL",
    });
  } catch {
    if (!kernel) return null;
  }
  const row = parsePsTable(text).find((x) => x.pid === pid);
  if (!row && !kernel) return null;
  let args = "unavailable";
  try {
    args = psImpl("ps", ["-o", "args=", "-p", String(pid)], {
      env,
      timeout: 1500,
      killSignal: "SIGKILL",
    }).trim();
  } catch {
    args = "unavailable";
  }
  if (kernel) {
    const latest = linuxBirth(pid);
    if (!latest) return null;
    if (!sameLinuxBirth(first, latest))
      return { ...latest, birthUnverifiable: true, error: "birth raced during ps observation" };
    return { ...row, ...latest, args, observedAt: isoNow() };
  }
  return { ...row, args, observedAt: isoNow() };
}

const comparableArgs = (a) => (typeof a === "string" && a !== "unavailable" ? a : null);

/**
 * Linux uses PID, kernel boot ID and start ticks. Other platforms use
 * pid + pgid + lstart, plus the `ps -o args=` string
 * when both sides recorded it. `comm` is informational only: on Linux, Node
 * renames its main thread (PR_SET_NAME) shortly after spawn, so procps
 * `comm` changes from "node" to "MainThread" for the same process.
 */
export function sameBirth(expected, actual) {
  if (!expected || !actual) return false;
  if (expected.birthVersion || actual.birthVersion) return sameLinuxBirth(expected, actual);
  if (actual.pid !== expected.pid || actual.pgid !== expected.pgid) return false;
  if (actual.lstart !== expected.lstart) return false;
  const want = comparableArgs(expected.args);
  const got = comparableArgs(actual.args);
  return want === null || got === null || want === got;
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
    if (birth && !birth.birthUnverifiable) return { birth, attempts: i + 1 };
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
          birthVersion: process.platform === "linux" ? LINUX_BIRTH : undefined,
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

/** snapshotOwned plus each process's full args (one `ps -o args=` per pid; "unavailable" if gone). */
export function snapshotOwnedWithArgs(handle, psImpl = run, platform = process.platform) {
  const env = { ...process.env, LC_ALL: "C" };
  return snapshotOwned(handle, psImpl, platform).map((p) => {
    let args = "unavailable";
    try {
      args =
        psImpl("ps", ["-o", "args=", "-p", String(p.pid)], {
          env,
          timeout: 1500,
          killSignal: "SIGKILL",
        }).trim() || "unavailable";
    } catch {
      args = "unavailable";
    }
    return { ...p, args };
  });
}

export function snapshotOwned(handle, psImpl, platform = process.platform, readLinux = linuxBirth) {
  if (!handle.native) return [];
  if (platform === "linux" && handle.native.birthVersion !== LINUX_BIRTH)
    throw new PidReuseError(handle.native, readLinux(handle.native.pid));
  const table = psTable(psImpl);
  if (handle.native.birthVersion === LINUX_BIRTH) {
    const root = handle.native;
    const leader = readLinux(root.pid);
    if (!leader) return [];
    if (!sameLinuxBirth(root, leader) || root.pgid !== leader.pgid || root.pgid !== root.pid)
      throw new PidReuseError(root, leader);
    const observed = ownedProcesses(table, root)
      .map((row) => {
        const now = readLinux(row.pid);
        if (!now) return null;
        if (now.birthUnverifiable) throw new PidReuseError(row, now);
        return { ...row, ...now };
      })
      .filter(Boolean);
    // Rebuild ancestry from kernel observations. A ps candidate may have exited/reparented.
    const seen = ownedProcesses(observed, root);
    const after = readLinux(root.pid);
    if (!after) return [];
    if (!sameLinuxBirth(root, after) || after.pgid !== root.pgid)
      throw new PidReuseError(root, after);
    for (const p of seen) {
      const old = handle.known?.get(p.pid);
      if (old && !sameBirth(old, p)) throw new PidReuseError(old, p);
      remember(handle, p, "snapshot");
    }
    return seen;
  }
  let seen = ownedProcesses(table, handle.native);
  if (handle.native.birthUnverifiable)
    seen = seen.filter((p) => inSpawnScope(handle.native, p, platform));
  for (const p of seen) remember(handle, p, "snapshot");
  return seen;
}

/** Members of `root`'s group within its spawn scope (used when the root birth is unverifiable). */
export function pgidScopeSince(table, root, platform = process.platform) {
  return table.filter((p) => inSpawnScope(root, p, platform));
}

/** Live owned processes whose birth still matches a known entry (async ps; ps -p exits 1 when some are gone). */
export async function liveKnownAsync(handle, execImpl = execFileAsync) {
  const known = [...(handle.known?.values() ?? [])];
  if (known.length === 0) return [];
  if (handle.native?.birthVersion === LINUX_BIRTH) {
    return known.flatMap((expected) => {
      const actual = linuxBirth(expected.pid);
      if (!actual) return [];
      if (!sameLinuxBirth(expected, actual) || expected.pgid !== actual.pgid)
        throw new PidReuseError(expected, actual);
      return [actual];
    });
  }
  let text = "";
  try {
    text = (
      await execImpl(
        "ps",
        ["-o", "pid=,ppid=,pgid=,lstart=,comm=", "-p", known.map((k) => k.pid).join(",")],
        { env: { ...process.env, LC_ALL: "C" }, timeout: 1500, killSignal: "SIGKILL" },
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
    signal: abortSignal,
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
  if (root.birthVersion === LINUX_BIRTH || platform === "linux")
    return stopLinux(handle, report, {
      intWaitMs,
      termWaitMs,
      killWaitMs,
      killImpl,
      psImpl,
      abortSignal,
      birthImpl: birthImpl === birthIdentity ? linuxBirth : birthImpl,
    });
  report.pgidScans = [];
  const unverifiable = root.birthUnverifiable === true;
  if (unverifiable) {
    report.mode = "unverifiable-birth-pgid-scope";
    report.scope = {
      pgid: root.pgid,
      minLstartMs: spawnSecondMs(root),
      rootMinLstartMs: scopeFloorMs(root, root, platform),
      lstartSlackMs: lstartSlackMs(platform),
      platform,
      note: "root birth was never readable; only group members started at or after the spawn second are signalled, individually; the platform's lstart slack applies to the root pid (pid === pgid) only",
    };
  }
  const rootNow = handle.exit === null && !unverifiable ? birthImpl(root.pid) : null;
  if (handle.exit === null && rootNow !== null && !sameBirth(root, rootNow))
    throw new PidReuseError(root, rootNow);
  const rootAlive = handle.exit === null && rootNow !== null;
  if (rootAlive) snapshotOwned(handle, psImpl, platform);
  const current = () => {
    abortSignal?.throwIfAborted();
    const table = psTable(psImpl);
    if (unverifiable) {
      for (const p of pgidScopeSince(table, root, platform))
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
      abortSignal?.throwIfAborted();
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

/** Verify again after opening a pidfd so numeric reuse before delivery is harmless. */
export function signalLinuxOwned(expected, signal, group = false) {
  const helper = fileURLToPath(new URL("./linux-signal.py", import.meta.url));
  const result = JSON.parse(
    run(
      "python3",
      [
        "-I",
        helper,
        String(expected.pid),
        String(expected.pgid),
        expected.bootId,
        expected.startTicks,
        signal,
        group ? "group" : "process",
      ],
      { timeout: 1500, killSignal: "SIGKILL" },
    ),
  );
  if (result.status === "mismatch") throw new PidReuseError(expected, result.actual);
  if (result.status !== "gone" && result.status !== "sent") throw new Error("invalid pidfd result");
  return result.status;
}

/** Never adopt an unknown member after the group leader exits. */
async function stopLinux(
  handle,
  report,
  { intWaitMs, termWaitMs, killWaitMs, killImpl, psImpl, birthImpl, abortSignal },
) {
  const root = handle.native;
  const endings = new Map();
  let lastSignal = "before-stop";
  const fail = (expected, actual) => {
    throw Object.assign(new PidReuseError(expected, actual), {
      steps: [...report.steps],
      remaining: [actual ?? expected],
      phase: "stop",
      exit: handle.exit,
    });
  };
  function current() {
    abortSignal?.throwIfAborted();
    const leader = birthImpl(root.pid);
    if (
      leader &&
      (!sameLinuxBirth(root, leader) || leader.pgid !== root.pid || root.pgid !== root.pid)
    )
      fail(root, leader);
    if (leader && handle.exit === null) snapshotOwned(handle, psImpl, "linux", birthImpl);
    else {
      for (const row of psTable(psImpl).filter((p) => p.pgid === root.pgid)) {
        const now = birthImpl(row.pid);
        if (!now) continue;
        if (now.birthUnverifiable) fail(handle.known?.get(row.pid) ?? row, now);
        if (now.pgid !== root.pgid) continue;
        const known = handle.known?.get(row.pid);
        if (!known || !sameLinuxBirth(known, now)) fail(known ?? root, now);
      }
    }
    const left = [];
    for (const expected of handle.known?.values() ?? []) {
      const now = birthImpl(expected.pid);
      if (!now) {
        if (!endings.has(expected.pid))
          endings.set(expected.pid, { pid: expected.pid, endedAfter: lastSignal });
      } else if (!sameLinuxBirth(expected, now) || expected.pgid !== now.pgid) fail(expected, now);
      else left.push(now);
    }
    return left;
  }
  const waits = { SIGINT: intWaitMs, SIGTERM: termWaitMs, SIGKILL: killWaitMs };
  try {
    report.ownedBefore = current();
    report.rootAlive = report.ownedBefore.some((p) => p.pid === root.pid);
    report.pgidScans = [];
    for (const signal of STOP_SEQUENCE) {
      const left = current();
      if (!left.length && handle.exit !== null) break;
      const group = left.some((p) => p.pid === root.pid) && handle.exit === null;
      const targets = group ? [root, ...left.filter((p) => p.pgid !== root.pgid)] : left;
      for (const expected of targets) {
        abortSignal?.throwIfAborted();
        const now = birthImpl(expected.pid);
        if (!now) continue;
        if (!sameLinuxBirth(expected, now) || expected.pgid !== now.pgid) fail(expected, now);
        const isGroup = group && expected === root;
        const step = {
          at: isoNow(),
          signal,
          target: isGroup ? `group:${root.pgid}` : expected.pid,
        };
        report.steps.push(step);
        try {
          if (killImpl === process.kill)
            step.delivery = signalLinuxOwned(expected, signal, isGroup);
          else killImpl(isGroup ? -root.pgid : expected.pid, signal);
        } catch (error) {
          step.error = String(error.code ?? error);
          throw error;
        }
      }
      lastSignal = signal;
      const end = performance.now() + waits[signal];
      while (performance.now() < end && (current().length || handle.exit === null)) await delay(50);
    }
    report.remaining = current();
    report.absent = report.remaining.length === 0 && handle.exit !== null;
    if (endings.has(root.pid)) endings.get(root.pid).exit = handle.exit;
    report.endings = [...endings.values()];
    report.exit = handle.exit;
    report.known = [...(handle.known?.values() ?? [])];
    report.finishedAt = isoNow();
    return report;
  } catch (error) {
    error.steps ??= report.steps;
    error.remaining ??= [...(handle.known?.values() ?? [])];
    throw error;
  }
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

/** The whole second containing `native`'s spawn (ms). */
export function spawnSecondMs(native) {
  return Math.floor(native.spawnWallMs / 1000) * 1000;
}

/**
 * Earliest lstart (ms) `p` may show to count as started by `root`'s spawn.
 * The platform's lstart slack applies only to the root itself (pid === the
 * recorded root pid === its pgid); every other group member must show
 * lstart >= the spawn second, so an older stray member of a reused group
 * can never enter the scope.
 */
export function scopeFloorMs(root, p, platform = process.platform) {
  const isRoot = p.pid === root.pid && p.pid === root.pgid;
  return spawnSecondMs(root) - (isRoot ? lstartSlackMs(platform) : 0);
}

export function inSpawnScope(root, p, platform = process.platform) {
  return p.pgid === root.pgid && (parseLstart(p.lstart) ?? -1) >= scopeFloorMs(root, p, platform);
}

/**
 * A present Linux PID in an unresolved registry row always blocks admission,
 * including indeterminate/legacy births. A verified BSD row matches by birth identity. A row
 * whose birth was unverifiable at spawn matches by pid + pgid with a start at
 * or after its spawn second. On Linux, procps lstart can read up to 1 s early
 * (see lstartSlackMs), so that 1 s slack is allowed for a root row
 * (pid === pgid) only; any other row needs lstart >= the spawn second.
 */
export function liveOwnedFromRegistry(
  entries,
  birthImpl = birthIdentity,
  platform = process.platform,
) {
  return entries.filter((e) => {
    const now = birthImpl(e.pid);
    if (!now) return false;
    if (platform === "linux") return true;
    if (e.birthUnverifiable) return inSpawnScope(e, { ...now, pid: e.pid }, platform);
    return sameBirth(e, now);
  });
}
