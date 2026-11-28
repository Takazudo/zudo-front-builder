import { timed } from "./controller-timing.mjs";
import { createHash, randomBytes } from "node:crypto";
import { execFileSync } from "node:child_process";
import {
  chmodSync,
  closeSync,
  cpSync,
  existsSync,
  fsyncSync,
  lstatSync,
  openSync,
  readdirSync,
  readFileSync,
  renameSync,
  writeSync,
} from "node:fs";
import { dirname, join, relative, sep } from "node:path";
import { fileURLToPath } from "node:url";

export const HARNESS_DIR = dirname(dirname(fileURLToPath(import.meta.url)));
export const HARNESS_VERSION = "dev-browser-observe.v1";

const immediate = () => new Promise((resolve) => setImmediate(resolve));

/**
 * Let libuv poll pending I/O between synchronous controller blocks. One
 * setImmediate called from the poll phase runs in that same turn's check phase
 * without polling; a second one is queued from check and runs only after the
 * next poll, so two always cross a poll phase whichever phase the caller is in.
 */
export async function yieldToLoop() {
  await immediate();
  await immediate();
}

export function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

export function sha256File(path) {
  return sha256(readFileSync(path));
}

export function randomHex(bytes = 6) {
  return randomBytes(bytes).toString("hex");
}

export function toPosix(path) {
  return path.split(sep).join("/");
}

/**
 * Deterministic hash over a set of `{ path, sha256 }` rows: rows are sorted by
 * path (byte order) and hashed as `<path>\0<sha256>\n`.
 */
export function treeHashFromEntries(entries) {
  const sorted = [...entries].sort((a, b) => (a.path < b.path ? -1 : a.path > b.path ? 1 : 0));
  const h = createHash("sha256");
  for (const e of sorted) h.update(`${e.path}\0${e.sha256}\n`);
  return { sha256: h.digest("hex"), fileCount: sorted.length, files: sorted };
}

/** Walk regular files (symlinks are skipped, never followed). */
export function walkFiles(root, { filter = () => true } = {}) {
  const out = [];
  const visit = (dir) => {
    for (const name of readdirSync(dir).sort()) {
      const abs = join(dir, name);
      const rel = toPosix(relative(root, abs));
      const st = lstatSync(abs);
      if (st.isDirectory()) {
        if (filter(rel, "dir", name)) visit(abs);
      } else if (st.isFile()) {
        if (filter(rel, "file", name)) out.push({ path: rel, abs });
      }
    }
  };
  visit(root);
  return out;
}

export function treeHashDir(root, opts = {}) {
  const files = walkFiles(root, opts).map((f) => ({ path: f.path, sha256: sha256File(f.abs) }));
  return treeHashFromEntries(files);
}

/** Append one JSON line and fsync before returning. */
export function appendJsonlSync(path, record, timing) {
  const fd = timed(timing, "open", {}, () => openSync(path, "a"));
  try {
    const line = JSON.stringify(record) + "\n";
    timed(timing, "write", {}, () => writeSync(fd, line));
    timed(timing, "fsync", {}, () => fsyncSync(fd));
  } finally {
    timed(timing, "close", {}, () => closeSync(fd));
  }
}

export function readJsonl(path) {
  if (!existsSync(path)) return [];
  return readFileSync(path, "utf8")
    .split("\n")
    .filter((l) => l.trim() !== "")
    .map((l) => JSON.parse(l));
}

/** Write JSON via temp file + fsync + rename. */
export function writeJsonSync(path, value) {
  const tmp = `${path}.tmp-${process.pid}`;
  const fd = openSync(tmp, "w");
  try {
    writeSync(fd, `${JSON.stringify(value, null, 2)}\n`);
    fsyncSync(fd);
  } finally {
    closeSync(fd);
  }
  renameSync(tmp, path);
}

export function readJson(path) {
  return JSON.parse(readFileSync(path, "utf8"));
}

export function run(cmd, args, opts = {}) {
  return execFileSync(cmd, args, {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
    maxBuffer: 256 * 1024 * 1024,
    ...opts,
  });
}

export function tryRun(cmd, args, opts = {}) {
  try {
    return { ok: true, stdout: run(cmd, args, opts) };
  } catch (error) {
    return {
      ok: false,
      error: String(error?.message ?? error),
      stdout: error?.stdout?.toString?.() ?? "",
    };
  }
}

export function gitHead(cwd) {
  return run("git", ["rev-parse", "HEAD"], { cwd }).trim();
}

/** Toplevel and HEAD from a single git process. */
export function gitToplevelAndHead(cwd) {
  const [top, head] = run("git", ["rev-parse", "--show-toplevel", "HEAD"], { cwd })
    .trim()
    .split("\n");
  return { top, head };
}

export const NO_WRITE_STEPS = "no-write-steps";

/** A workload with no write steps: a plan error, distinct from a failed write (nothing was written). */
export function noWriteStepsError() {
  const error = new Error("no write steps");
  error.code = NO_WRITE_STEPS;
  error.steps = [];
  error.failedStep = null;
  return error;
}

export function gitToplevel(cwd) {
  return run("git", ["rev-parse", "--show-toplevel"], { cwd }).trim();
}

/** `git diff HEAD` bytes plus untracked paths; dirty when either is nonempty. */
export function gitDirtyState(cwd, pathspec = []) {
  const patch = execFileSync("git", ["diff", "--binary", "HEAD", "--", ...pathspec], {
    cwd,
    maxBuffer: 1024 * 1024 * 1024,
  });
  const untracked = run("git", ["ls-files", "--others", "--exclude-standard", "--", ...pathspec], {
    cwd,
  })
    .split("\n")
    .filter(Boolean);
  return {
    patchSha256: sha256(patch),
    patchBytes: patch.length,
    untracked,
    dirty: patch.length > 0 || untracked.length > 0,
  };
}

export function hrtimeNs() {
  return process.hrtime.bigint();
}

export function nsToMs(ns) {
  return Number(ns) / 1e6;
}

export function stripAnsi(text) {
  // eslint-disable-next-line no-control-regex
  return text.replace(/\u001b\[[0-9;]*[A-Za-z]/g, "");
}

export function isoNow() {
  return new Date().toISOString();
}

export function delay(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

/**
 * Give the owner write access to a copied tree (dirs u+rwx, files u+rw), without following
 * symlinks and without touching other mode bits. cpSync keeps source modes, so a copy of a
 * read-only (for example frozen) fixture would otherwise be a work dir nothing can write into.
 * Tree hashes cover bytes and paths only, so this changes no identity hash.
 */
export function ensureOwnerWritable(root) {
  const visit = (p) => {
    const st = lstatSync(p);
    if (st.isSymbolicLink()) return;
    const want = st.isDirectory() ? 0o700 : 0o600;
    if ((st.mode & want) !== want) chmodSync(p, (st.mode & 0o7777) | want);
    if (st.isDirectory()) for (const name of readdirSync(p)) visit(join(p, name));
  };
  visit(root);
}

/** cpSync into a work dir, then make the copy owner-writable (ensureOwnerWritable). */
export function cpSyncWritable(src, dest, opts) {
  cpSync(src, dest, opts);
  ensureOwnerWritable(dest);
}
