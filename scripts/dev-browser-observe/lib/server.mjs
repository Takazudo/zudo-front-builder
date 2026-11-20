import { createWriteStream } from "node:fs";
import { sample, sampleAsync } from "./mac-process-adapter.mjs";
import { birthIdentity, sameBirth, snapshotOwned, spawnOwned } from "./process-owner.mjs";
import { hrtimeNs, isoNow, nsToMs, stripAnsi } from "./util.mjs";

// crates/zfb/src/output.rs fmt_ready prints "→ ready on <url>" once routes are
// published; "listening on … (routes pending)" is the earlier bind signal.
export const READY_RE = /ready on (http:\/\/\S+)/;

// Watch/tick diagnostics zfb prints only under ZFB_DEV_TIMING=1.
export const DIAGNOSTIC_ENV = ["ZFB_DEV_TIMING"];

export const SERVER_LOG_LINE_CAP = 400;

export function parseReadyUrl(text) {
  const m = stripAnsi(text).match(READY_RE);
  return m ? m[1].replace(/\/$/, "") : null;
}

export function devArgs({ port, host, extraArgs = [] }) {
  const p = port.mode === "fixed" ? String(port.port) : "0";
  return ["dev", "--port", p, "--host", host, ...extraArgs];
}

/**
 * Effective child env. Outside diagnostic profiles every DIAGNOSTIC_ENV key is
 * removed, including one inherited from the controller's own environment.
 */
export function serverEnv(baseEnv, overrides, diagnostic) {
  const env = { ...baseEnv, NO_COLOR: "1", ...overrides };
  const removed = [];
  if (!diagnostic) {
    for (const k of DIAGNOSTIC_ENV) {
      if (k in env) {
        removed.push(k);
        delete env[k];
      }
    }
  }
  const zfbEnv = Object.fromEntries(Object.entries(env).filter(([k]) => k.startsWith("ZFB_")));
  return { env, removedDiagnosticEnv: removed, zfbEnv };
}

/** Negative control: SIGKILL the owned native process after re-verifying its birth identity. */
export function injectKillOwned(handle, killImpl = process.kill) {
  const root = handle.native;
  const now = handle.exit === null ? birthIdentity(root.pid) : null;
  if (!sameBirth(root, now))
    return { ok: false, reason: "birth-identity-mismatch-or-gone", expected: root, actual: now };
  killImpl(root.pid, "SIGKILL");
  return { ok: true, pid: root.pid, signal: "SIGKILL", at: isoNow(), birth: now };
}

export function startDevServer({
  exe,
  appDir,
  port,
  host = "127.0.0.1",
  extraArgs = [],
  env = {},
  diagnostic = false,
  readyTimeoutMs,
  logPath,
}) {
  const log = createWriteStream(logPath, { flags: "a" });
  const args = devArgs({ port, host, extraArgs });
  const effective = serverEnv(process.env, env, diagnostic);
  const lines = [];
  let combined = "";
  let resolveReady = null;
  const writeLog = (stream) => (chunk) => {
    const text = chunk.toString();
    const ns = hrtimeNs();
    for (const line of text.split(/(?<=\n)/)) {
      if (line === "") continue;
      log.write(`[${isoNow()}] [${stream}] ${line}${line.endsWith("\n") ? "" : "\n"}`);
      lines.push({ ns, stream, text: stripAnsi(line).replace(/\n$/, "") });
    }
    combined += text;
    if (resolveReady) {
      const url = parseReadyUrl(combined);
      if (url) resolveReady(url);
    }
  };
  const handle = spawnOwned(exe, args, {
    cwd: appDir,
    env: effective.env,
    onStdout: writeLog("stdout"),
    onStderr: writeLog("stderr"),
  });
  const startedNs = hrtimeNs();
  const ready = new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      resolveReady = null;
      reject(
        Object.assign(new Error(`zfb dev not ready within ${readyTimeoutMs} ms`), {
          code: "READY_TIMEOUT",
        }),
      );
    }, readyTimeoutMs);
    resolveReady = (url) => {
      resolveReady = null;
      clearTimeout(timer);
      resolve({ baseUrl: url, readyNs: hrtimeNs() });
    };
    const already = parseReadyUrl(combined);
    if (already) resolveReady(already);
    handle.exited.then((exit) => {
      if (!resolveReady) return;
      resolveReady = null;
      clearTimeout(timer);
      reject(
        Object.assign(new Error(`zfb dev exited before ready: ${JSON.stringify(exit)}`), {
          code: "EXIT_BEFORE_READY",
          exit,
        }),
      );
    });
  });
  return {
    handle,
    args,
    envOverrides: { NO_COLOR: "1", ...env },
    removedDiagnosticEnv: effective.removedDiagnosticEnv,
    zfbEnv: effective.zfbEnv,
    diagnostic,
    startedNs,
    ready,
    log,
    isAlive: () => ({ alive: handle.exit === null, exit: handle.exit }),
    pid: () => handle.native?.pid ?? null,
    waitExit: () => handle.exited,
    sampleResources: () => {
      const owned = handle.exit === null ? snapshotOwned(handle) : [];
      return sample(owned.map((p) => p.pid));
    },
    sampleResourcesAsync: () => sampleAsync([...(handle.known?.keys() ?? [])]),
    injectKill: () => injectKillOwned(handle),
    logMark: () => lines.length,
    /** Server output lines since `mark`, each with ms relative to the attempt's t0 (server.log has them all). */
    logSince: (mark, t0) => {
      const slice = lines.slice(mark);
      return {
        lineCount: slice.length,
        truncated: slice.length > SERVER_LOG_LINE_CAP,
        lines: slice
          .slice(0, SERVER_LOG_LINE_CAP)
          .map((l) => ({ msFromT0: nsToMs(l.ns - t0), stream: l.stream, text: l.text })),
      };
    },
  };
}
