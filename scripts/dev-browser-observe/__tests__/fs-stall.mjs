import fs from "node:fs";
import { syncBuiltinESMExports } from "node:module";

const blockSync = (ms) => Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms);

// Storage-stall injector for the durability replays (design §4). arm(match, ms)
// makes the next fsync of the fd that receives a write containing `match` take
// `ms`: fsyncSync blocks the event loop (today's writer), while an async fsync
// completes late on the threadpool side and leaves the loop free (durable mode).
// Delays armed on the same fd before its fsync add up.
export function stallFs() {
  const rules = [];
  const armed = new Map();
  const log = [];
  const saved = { writeSync: fs.writeSync, fsyncSync: fs.fsyncSync, fsync: fs.fsync };
  fs.writeSync = (fd, data, ...rest) => {
    if (typeof data === "string")
      for (const rule of rules)
        if (!rule.used && data.includes(rule.match)) {
          rule.used = true;
          armed.set(fd, (armed.get(fd) ?? 0) + rule.ms);
        }
    return saved.writeSync(fd, data, ...rest);
  };
  fs.fsyncSync = (fd) => {
    const ms = armed.get(fd);
    if (ms) {
      armed.delete(fd);
      const startNs = process.hrtime.bigint();
      blockSync(ms);
      log.push({ kind: "fsyncSync", ms, startNs, endNs: process.hrtime.bigint() });
    }
    return saved.fsyncSync(fd);
  };
  fs.fsync = (fd, cb) => {
    const ms = armed.get(fd);
    if (!ms) return saved.fsync(fd, cb);
    armed.delete(fd);
    const startNs = process.hrtime.bigint();
    setTimeout(
      () =>
        saved.fsync(fd, (error) => {
          log.push({ kind: "fsync", ms, startNs, endNs: process.hrtime.bigint() });
          cb(error);
        }),
      ms,
    );
  };
  syncBuiltinESMExports();
  return {
    log,
    arm(match, ms) {
      rules.push({ match, ms, used: false });
    },
    stop() {
      Object.assign(fs, saved);
      syncBuiltinESMExports();
    },
  };
}
