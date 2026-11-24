import { createWriteStream } from "node:fs";
import { StringDecoder } from "node:string_decoder";
import { sample, sampleAsync } from "./mac-process-adapter.mjs";
import {
  birthIdentity,
  liveKnownAsync,
  sameBirth,
  signalLinuxOwned,
  snapshotOwned,
  spawnOwned,
} from "./process-owner.mjs";
import { hrtimeNs, isoNow, nsToMs, stripAnsi } from "./util.mjs";

// crates/zfb/src/output.rs fmt_ready prints "→ ready on <url>" once routes are
// published; "listening on … (routes pending)" is the earlier bind signal.
// Matched only against complete lines, so a URL split across chunks never matches truncated.
export const READY_RE = /ready on (http:\/\/\S+)\s*$/;

export const SERVER_LINES_KEPT = 20000;

// Diagnostics zfb prints only when these are set: watch/tick timing (ZFB_DEV_TIMING=1) and
// `[zfb-prov] {json}` provenance lines (ZFB_DEV_PROVENANCE_TRACE).
export const DIAGNOSTIC_ENV = ["ZFB_DEV_TIMING", "ZFB_DEV_PROVENANCE_TRACE"];

export const SERVER_LOG_LINE_CAP = 400;

// `[zfb-prov]` trace lines get their own buffer, so neither the 400-line record
// cap nor the general line cap can drop them silently; any drop is reported.
export const PROV_LINES_KEPT = 500000;
const PROV_MARKER = "[zfb-prov] ";

/** Ready URL from complete lines of `text` (an unterminated last line is ignored). */
export function parseReadyUrl(text) {
  const complete = stripAnsi(text).split(/\r?\n/).slice(0, -1);
  for (const line of complete) {
    const m = line.match(READY_RE);
    if (m) return m[1].replace(/\/$/, "");
  }
  return null;
}

/** Splits a byte stream into complete lines; the unterminated tail waits for more data. */
export class LineSplitter {
  constructor() {
    this.tail = "";
    this.decoder = new StringDecoder("utf8");
  }

  push(chunk) {
    const text = typeof chunk === "string" ? chunk : this.decoder.write(chunk);
    const parts = (this.tail + text).split(/\r?\n/);
    this.tail = parts.pop();
    return parts;
  }

  flush() {
    const rest = this.tail + this.decoder.end();
    this.tail = "";
    return rest === "" ? [] : [rest];
  }
}

/**
 * server.log writer that never throws: writes after close are counted and
 * dropped (child pipes can still deliver data after the process is stopped),
 * and stream errors are recorded instead of crashing the controller.
 */
export class ServerLog {
  constructor(stream) {
    this.stream = stream;
    this.ended = false;
    this.droppedAfterEnd = 0;
    this.errors = [];
    stream.on("error", (error) => this.errors.push(String(error?.message ?? error)));
  }

  write(text) {
    if (this.ended) {
      this.droppedAfterEnd += 1;
      return;
    }
    this.stream.write(text);
  }

  end() {
    if (this.ended) return Promise.resolve();
    this.ended = true;
    return new Promise((resolve) => this.stream.end(resolve));
  }
}

/** Resolve when both child pipes closed, or after `ms`; never rejects. */
export function pipesClosed(child, ms) {
  const one = (s) =>
    !s || s.destroyed || s.closed ? Promise.resolve() : new Promise((r) => s.once("close", r));
  return Promise.race([
    Promise.all([one(child.stdout), one(child.stderr)]).then(() => "closed"),
    new Promise((r) => setTimeout(() => r("timeout"), ms)),
  ]);
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
  if (root == null) return { ok: false, reason: "never-spawned" };
  const now = handle.exit === null ? birthIdentity(root.pid) : null;
  if (!sameBirth(root, now) || root.pgid !== now.pgid)
    return { ok: false, reason: "birth-identity-mismatch-or-gone", expected: root, actual: now };
  if (root.birthVersion && killImpl === process.kill) signalLinuxOwned(root, "SIGKILL");
  else killImpl(root.pid, "SIGKILL");
  return { ok: true, pid: root.pid, signal: "SIGKILL", at: isoNow(), birth: now };
}

/**
 * Spawn the dev server as an owned detached group and resolve `ready` from its output.
 * Source-built: `exe` + devArgs, ready on READY_RE. Fixed-distribution: an explicit `command`
 * and `args` with a `readiness` object (distribution.mjs createReadiness) whose `push(line)`
 * returns { url } once ready or { failed } for a readiness contradiction (wrong port).
 */
export function startDevServer({
  exe,
  command = null,
  args: explicitArgs = null,
  readiness = null,
  appDir,
  port,
  host = "127.0.0.1",
  extraArgs = [],
  env = {},
  diagnostic = false,
  readyTimeoutMs,
  logPath,
  spawnImpl = spawnOwned,
}) {
  const log = new ServerLog(createWriteStream(logPath, { flags: "a" }));
  const args = explicitArgs ?? devArgs({ port, host, extraArgs });
  const cmd = command ?? exe;
  let rejectReady = null;
  const effective = serverEnv(process.env, env, diagnostic);
  const lines = [];
  let linesBase = 0;
  const provLines = [];
  let provBase = 0;
  let resolveReady = null;
  const splitters = { stdout: new LineSplitter(), stderr: new LineSplitter() };
  const take = (stream, line, ns) => {
    log.write(`[${isoNow()}] [${stream}] ${line}\n`);
    const text = stripAnsi(line);
    lines.push({ ns, stream, text });
    if (text.includes(PROV_MARKER)) {
      provLines.push({ ns, stream, text });
      if (provLines.length > PROV_LINES_KEPT) {
        const drop = provLines.length - PROV_LINES_KEPT;
        provLines.splice(0, drop);
        provBase += drop;
      }
    }
    if (lines.length > SERVER_LINES_KEPT) {
      const drop = lines.length - SERVER_LINES_KEPT;
      lines.splice(0, drop);
      linesBase += drop;
    }
    if (resolveReady && readiness) {
      const r = readiness.push(text);
      if (r?.failed) rejectReady(r.failed);
      else if (r?.url) resolveReady(r.url);
    } else if (resolveReady) {
      const m = text.match(READY_RE);
      if (m) resolveReady(m[1].replace(/\/$/, ""));
    }
  };
  const writeLog = (stream) => (chunk) => {
    const ns = hrtimeNs();
    for (const line of splitters[stream].push(chunk)) take(stream, line, ns);
  };
  const handle = spawnImpl(cmd, args, {
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
    rejectReady = (failure) => {
      resolveReady = null;
      clearTimeout(timer);
      reject(
        Object.assign(new Error(`dev server readiness failed: ${JSON.stringify(failure)}`), {
          code: "READY_CONTRADICTED",
          failure,
        }),
      );
    };
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
    /** Settle pending pipe data (bounded), flush unterminated tails, then end server.log. */
    closeLog: async (ms = 2000) => {
      const pipes = await pipesClosed(handle.child, ms);
      const ns = hrtimeNs();
      for (const stream of ["stdout", "stderr"])
        for (const line of splitters[stream].flush()) take(stream, line, ns);
      await log.end();
      return { pipes, droppedAfterEnd: log.droppedAfterEnd, errors: log.errors };
    },
    isAlive: () => ({ alive: handle.exit === null, exit: handle.exit }),
    pid: () => handle.native?.pid ?? null,
    waitExit: () => handle.exited,
    sampleResources: () => {
      const owned = handle.exit === null ? snapshotOwned(handle) : [];
      return sample(owned);
    },
    /** Only owned pids whose birth identity still matches are sampled. */
    sampleResourcesAsync: async (opts = {}) => {
      const live = await liveKnownAsync(handle);
      return {
        ...(await sampleAsync(live, opts)),
        birthVerified: live.length,
      };
    },
    injectKill: () => injectKillOwned(handle),
    logMark: () => linesBase + lines.length,
    provMark: () => provBase + provLines.length,
    /** Every `[zfb-prov]` line since `mark` (uncapped); droppedBeforeMark counts buffer loss. */
    provSince: (mark) => {
      const start = mark - provBase;
      return {
        lines: provLines.slice(Math.max(start, 0)).map((l) => ({ ns: l.ns, text: l.text })),
        droppedBeforeMark: start < 0 ? -start : 0,
      };
    },
    /** Server output lines since `mark`, each with ms relative to the attempt's t0 (server.log has them all). */
    logSince: (mark, t0) => {
      const start = mark - linesBase;
      const slice = lines.slice(Math.max(start, 0));
      return {
        lineCount: slice.length,
        droppedBeforeMark: start < 0 ? -start : 0,
        truncated: slice.length > SERVER_LOG_LINE_CAP,
        lines: slice
          .slice(0, SERVER_LOG_LINE_CAP)
          .map((l) => ({ msFromT0: nsToMs(l.ns - t0), stream: l.stream, text: l.text })),
      };
    },
  };
}
