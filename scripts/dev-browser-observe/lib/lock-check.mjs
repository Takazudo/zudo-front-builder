import { spawn } from "node:child_process";
import { existsSync, mkdirSync } from "node:fs";
import { join } from "node:path";
import { LockBusyError, acquireLock } from "./lock.mjs";
import { birthIdentity, sameBirth } from "./process-owner.mjs";
import { loadProfile } from "./profile.mjs";
import { findGitWorkTree } from "./run.mjs";
import { HARNESS_DIR, randomHex } from "./util.mjs";

const CLI = join(HARNESS_DIR, "cli.mjs");

/** Hold the profile's batch lock until `ms` elapse or stdin closes. */
export async function holdLock(
  profilePath,
  ms,
  { out = (m) => process.stdout.write(m), stdin = process.stdin } = {},
) {
  const { resolved } = loadProfile(profilePath);
  const gitTree = findGitWorkTree(resolved.outDir);
  if (gitTree) {
    out(`REFUSED outDir is inside the git work tree ${gitTree}\n`);
    return 2;
  }
  mkdirSync(resolved.outDir, { recursive: true });
  let lock;
  try {
    lock = await acquireLock(resolved.lockPath);
  } catch (error) {
    if (error instanceof LockBusyError) {
      out(`BUSY ${resolved.lockPath}\n`);
      return 5;
    }
    out(`ERROR ${error?.message ?? error}\n`);
    return 8;
  }
  out(`LOCKED ${resolved.lockPath}\n`);
  await new Promise((resolve) => {
    const done = () => {
      clearTimeout(timer);
      stdin.removeListener("end", done);
      stdin.removeListener("close", done);
      stdin.pause();
      stdin.unref?.();
      resolve();
    };
    const timer = setTimeout(done, ms);
    stdin.once("end", done);
    stdin.once("close", done);
    stdin.resume();
  });
  await lock.release();
  return 0;
}

export function runNode(args, { onLine } = {}) {
  const child = spawn(process.execPath, [CLI, ...args], { stdio: ["pipe", "pipe", "pipe"] });
  let stdout = "";
  let stderr = "";
  child.stdout.on("data", (d) => {
    stdout += d.toString();
    if (onLine) onLine(stdout);
  });
  child.stderr.on("data", (d) => {
    stderr += d.toString();
  });
  const done = new Promise((resolve) =>
    child.once("exit", (code, signal) => resolve({ code, signal, stdout, stderr })),
  );
  return { child, done };
}

/**
 * Lock-contention negative control through the real CLI: one child holds the
 * lock, a second `run` must exit 5 without creating its label dir. The whole
 * check is bounded by `deadlineMs`: on expiry every child still running is
 * killed (only after its birth identity re-verifies) and the result is
 * `{ ok: false, timedOut: true }`.
 */
export async function lockContentionCheck(
  profilePath,
  {
    holdMs = 60000,
    deadlineMs = 120000,
    killWaitMs = 3000,
    runNodeImpl = runNode,
    birthImpl = birthIdentity,
    killImpl = process.kill,
  } = {},
) {
  const { resolved } = loadProfile(profilePath);
  const label = `lock-contention-${randomHex(4)}`;
  const children = [];
  let timedOut = false;
  const track = (role, started) => {
    const entry = { role, ...started, birth: birthImpl(started.child.pid), exited: false };
    started.done.then(() => {
      entry.exited = true;
    });
    children.push(entry);
    return started;
  };
  let deadlineTimer;
  const deadline = new Promise((resolve) => {
    deadlineTimer = setTimeout(() => {
      timedOut = true;
      resolve({ timedOut: true });
    }, deadlineMs);
  });
  const check = async () => {
    let lockedResolve;
    const locked = new Promise((r) => {
      lockedResolve = r;
    });
    const holder = track(
      "holder",
      runNodeImpl(["hold-lock", "--profile", profilePath, "--ms", String(holdMs)], {
        onLine: (text) => {
          if (/^(LOCKED|BUSY|ERROR|REFUSED)/m.test(text)) lockedResolve(text);
        },
      }),
    );
    const holderState = await Promise.race([
      locked,
      holder.done.then((r) => `EXITED ${JSON.stringify(r)}`),
    ]);
    if (!String(holderState).startsWith("LOCKED")) {
      holder.child.stdin.end();
      return { ok: false, reason: "holder-could-not-lock", holderState: String(holderState) };
    }
    if (timedOut) return null;
    const second = await track(
      "second-run",
      runNodeImpl(["run", "--profile", profilePath, "--label", label]),
    ).done;
    holder.child.stdin.end();
    const holderExit = await holder.done;
    const labelDir = join(resolved.outDir, label);
    const ok = second.code === 5 && !existsSync(labelDir) && holderExit.code === 0;
    return {
      ok,
      expected: { secondRunExit: 5, labelDirCreated: false },
      holder: { pid: holder.child.pid, exit: { code: holderExit.code, signal: holderExit.signal } },
      secondRun: { label, exit: second.code, stderr: second.stderr.trim() },
      labelDirCreated: existsSync(labelDir),
      lockPath: resolved.lockPath,
    };
  };
  const result = await Promise.race([check(), deadline]);
  clearTimeout(deadlineTimer);
  if (!result?.timedOut) return result;
  const killed = [];
  for (const c of children) {
    if (c.exited) continue;
    const now = birthImpl(c.child.pid);
    if (!c.birth || !sameBirth(c.birth, now)) {
      killed.push({ role: c.role, pid: c.child.pid, killed: false, reason: "birth-unverified" });
      continue;
    }
    killImpl(c.child.pid, "SIGKILL");
    const exit = await Promise.race([
      c.done,
      new Promise((r) => setTimeout(() => r(null), killWaitMs)),
    ]);
    killed.push({
      role: c.role,
      pid: c.child.pid,
      killed: true,
      signal: "SIGKILL",
      exitSeen: exit !== null,
    });
  }
  return {
    ok: false,
    timedOut: true,
    deadlineMs,
    children: killed,
    lockPath: resolved.lockPath,
  };
}
