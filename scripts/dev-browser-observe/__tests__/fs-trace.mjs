import fs from "node:fs";
import { syncBuiltinESMExports } from "node:module";
import { relative } from "node:path";

// Values whose identity is part of the flag-off contract; every other leaf is
// reduced to its JSON type so host, clock and temp-path differences drop out.
const KEPT = new Set(["type", "outcome", "label", "phase", "status", "reason", "stopReason", "op"]);

export function shapeOf(value, key = null) {
  if (Array.isArray(value)) return value.map((v) => shapeOf(v));
  if (value && typeof value === "object")
    return Object.fromEntries(Object.entries(value).map(([k, v]) => [k, shapeOf(v, k)]));
  if (value === null || typeof value === "boolean") return value;
  if (typeof value === "string" && KEPT.has(key)) return value;
  return "<" + typeof value + ">";
}

const fileName = (root, path) => relative(root, String(path)).replace(/\.tmp-\d+(-\d+)?$/, ".tmp");

// Traces the node:fs calls that write durable evidence under `root`: open,
// write (JSONL lines reduced to their record shape), fsync (sync and async),
// close, rename and unlink. `stop()` restores the originals.
export function traceFs(root) {
  const events = [];
  const paths = new Map();
  const saved = {};
  const under = (p) => typeof p === "string" && p.startsWith(root);
  const wrap = (name, fn) => {
    saved[name] = fs[name];
    fs[name] = fn(saved[name]);
  };
  wrap("openSync", (orig) => (path, ...rest) => {
    const fd = orig(path, ...rest);
    const flags = rest[0] ?? "r";
    if (under(path) && flags !== "r" && flags !== fs.constants.O_RDONLY) {
      paths.set(fd, String(path));
      events.push(["open", fileName(root, path), String(flags)]);
    }
    return fd;
  });
  wrap("writeSync", (orig) => (fd, data, ...rest) => {
    const path = paths.get(fd);
    if (path) {
      const text = typeof data === "string" ? data : Buffer.from(data).toString("utf8");
      const name = fileName(root, path);
      if (path.endsWith(".jsonl"))
        for (const line of text.split("\n").filter(Boolean))
          events.push(["write", name, shapeOf(JSON.parse(line))]);
      else events.push(["write", name]);
    }
    return orig(fd, data, ...rest);
  });
  wrap("fsyncSync", (orig) => (fd) => {
    if (paths.has(fd)) events.push(["fsyncSync", fileName(root, paths.get(fd))]);
    return orig(fd);
  });
  wrap("fsync", (orig) => (fd, cb) => {
    if (paths.has(fd)) events.push(["fsync", fileName(root, paths.get(fd))]);
    return orig(fd, cb);
  });
  wrap("closeSync", (orig) => (fd) => {
    if (paths.has(fd)) {
      events.push(["close", fileName(root, paths.get(fd))]);
      paths.delete(fd);
    }
    return orig(fd);
  });
  wrap("renameSync", (orig) => (from, to) => {
    if (under(from)) events.push(["rename", fileName(root, from), fileName(root, to)]);
    return orig(from, to);
  });
  wrap("unlinkSync", (orig) => (path) => {
    if (under(path)) events.push(["unlink", fileName(root, path)]);
    return orig(path);
  });
  syncBuiltinESMExports();
  return {
    events,
    stop() {
      Object.assign(fs, saved);
      syncBuiltinESMExports();
    },
  };
}
