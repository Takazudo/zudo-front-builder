import {
  closeSync,
  fsyncSync,
  mkdirSync,
  openSync,
  renameSync,
  unlinkSync,
  writeSync,
} from "node:fs";
import { userInfo } from "node:os";
import { dirname, join } from "node:path";
import { timed } from "./controller-timing.mjs";
import { readJson, writeJsonSync } from "./util.mjs";

// All Linux output directories and frozen harness copies share this intent.
// Reserve before spawning, so a crashed controller or failed receipt write
// leaves positive evidence that cleanup needs reconciliation.
export function admissionPath() {
  return join(userInfo().homedir, ".local/state/zfb/dev-browser-observe/active.json");
}

// With a durable sink (O5, opt-in profiles only), the reservation itself stays
// synchronous and also fsyncs its directory, because its existence is the hold a
// later run checks after a host crash (review re-check R2-4). Later updates are a
// write and rename without fsync; the next barrier fsyncs the file and directory.
export function reserveSession(path, metadata, { sink = null, timing = null } = {}) {
  mkdirSync(dirname(path), { recursive: true, mode: 0o700 });
  try {
    closeSync(openSync(path, "wx", 0o600));
  } catch (error) {
    if (error.code !== "EEXIST") throw error;
    let details;
    try {
      details = readJson(path);
    } catch {
      details = { state: "unreadable or incomplete" };
    }
    throw Object.assign(
      new Error(`unresolved benchmark session at ${path}: ${JSON.stringify(details)}`),
      {
        code: "ADMISSION_HOLD",
      },
    );
  }
  const state = { version: 1, controllerPid: process.pid, ...metadata };
  function update(fields) {
    Object.assign(state, fields);
    if (!sink) return writeJsonSync(path, state);
    timed(timing, "intent.update", { phase: state.phase ?? null }, () => {
      const tmp = `${path}.tmp-${process.pid}`;
      const fd = openSync(tmp, "w");
      try {
        writeSync(fd, `${JSON.stringify(state, null, 2)}\n`);
      } finally {
        closeSync(fd);
      }
      renameSync(tmp, path);
      sink.noteRenamed(path);
    });
  }
  Object.assign(state, { phase: "before-spawn" });
  timed(sink && timing, "intent.reserve", {}, () => {
    writeJsonSync(path, state);
    if (sink) {
      const dir = openSync(dirname(path), "r");
      try {
        fsyncSync(dir);
      } finally {
        closeSync(dir);
      }
    }
  });
  return { update, release: () => unlinkSync(path) };
}
