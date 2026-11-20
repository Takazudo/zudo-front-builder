import { createWriteStream } from "node:fs";
import { sample } from "./mac-process-adapter.mjs";
import { snapshotOwned, spawnOwned } from "./process-owner.mjs";
import { hrtimeNs, isoNow, stripAnsi } from "./util.mjs";

// crates/zfb/src/output.rs fmt_ready prints "→ ready on <url>" once routes are
// published; "listening on … (routes pending)" is the earlier bind signal.
export const READY_RE = /ready on (http:\/\/\S+)/;

export function parseReadyUrl(text) {
  const m = stripAnsi(text).match(READY_RE);
  return m ? m[1].replace(/\/$/, "") : null;
}

export function devArgs({ port, host, extraArgs = [] }) {
  const p = port.mode === "fixed" ? String(port.port) : "0";
  return ["dev", "--port", p, "--host", host, ...extraArgs];
}

export function startDevServer({
  exe,
  appDir,
  port,
  host = "127.0.0.1",
  extraArgs = [],
  env = {},
  readyTimeoutMs,
  logPath,
}) {
  const log = createWriteStream(logPath, { flags: "a" });
  const args = devArgs({ port, host, extraArgs });
  const fullEnv = { ...process.env, NO_COLOR: "1", ...env };
  let stdoutText = "";
  const writeLog = (stream) => (chunk) => {
    const text = chunk.toString();
    for (const line of text.split(/(?<=\n)/))
      log.write(`[${isoNow()}] [${stream}] ${line}${line.endsWith("\n") ? "" : "\n"}`);
    if (stream === "stdout") stdoutText += text;
  };
  const handle = spawnOwned(exe, args, {
    cwd: appDir,
    env: fullEnv,
    onStdout: writeLog("stdout"),
    onStderr: writeLog("stderr"),
  });
  const startedNs = hrtimeNs();
  const ready = new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      clearInterval(poll);
      reject(
        Object.assign(new Error(`zfb dev not ready within ${readyTimeoutMs} ms`), {
          code: "READY_TIMEOUT",
        }),
      );
    }, readyTimeoutMs);
    const poll = setInterval(() => {
      const url = parseReadyUrl(stdoutText);
      if (url) {
        clearInterval(poll);
        clearTimeout(timer);
        resolve({ baseUrl: url, readyNs: hrtimeNs() });
      }
    }, 20);
    handle.exited.then((exit) => {
      clearInterval(poll);
      clearTimeout(timer);
      reject(
        Object.assign(new Error(`zfb dev exited before ready: ${JSON.stringify(exit)}`), {
          code: "EXIT_BEFORE_READY",
          exit,
        }),
      );
    });
  });
  const server = {
    handle,
    args,
    envOverrides: { NO_COLOR: "1", ...env },
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
  };
  return server;
}
