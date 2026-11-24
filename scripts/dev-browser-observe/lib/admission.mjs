import { closeSync, mkdirSync, openSync, unlinkSync } from "node:fs";
import { userInfo } from "node:os";
import { dirname, join } from "node:path";
import { readJson, writeJsonSync } from "./util.mjs";

// All Linux output directories and frozen harness copies share this intent.
// Reserve before spawning, so a crashed controller or failed receipt write
// leaves positive evidence that cleanup needs reconciliation.
export function admissionPath() {
  return join(userInfo().homedir, ".local/state/zfb/dev-browser-observe/active.json");
}

export function reserveSession(path, metadata) {
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
    writeJsonSync(path, state);
  }
  update({ phase: "before-spawn" });
  return { update, release: () => unlinkSync(path) };
}
