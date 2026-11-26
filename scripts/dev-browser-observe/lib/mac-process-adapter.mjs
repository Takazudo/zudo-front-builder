import { timed } from "./controller-timing.mjs";
import { readFileSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { linuxBirth, parseLinuxStat, LINUX_BIRTH, sameLinuxBirth } from "./linux-birth.mjs";
import { execFile } from "node:child_process";
import { loadavg, totalmem, freemem, platform, arch, release } from "node:os";
import { promisify } from "node:util";
import { isoNow, run } from "./util.mjs";

const probeOptions = () => ({
  env: { ...process.env, LC_ALL: "C" },
  timeout: 1500,
  killSignal: "SIGKILL",
});

function safe(fn) {
  try {
    return fn();
  } catch {
    return null;
  }
}

export const SAMPLE_LABELS = {
  rss: "instantaneous resident set size in KiB from ps; summed across owned processes it is not unique memory",
  peak: "sampled peak over discrete samples, not a lifetime high-water mark",
  sigkill: "a SIGKILL exit is never interpreted as OOM or a leak",
};

function probeField(field, pid) {
  const out = safe(() => run("ps", ["-o", `${field}=`, "-p", String(pid)], probeOptions()).trim());
  return out === null || out === "" ? { works: false } : { works: true, example: out };
}

const BOOT_ID_PATH = "/proc/sys/kernel/random/boot_id";

function linuxThreadCount(expected, stat, bootId) {
  const row = parseLinuxStat(stat, expected.pid, { withThreads: true });
  const actual = { ...row, birthVersion: LINUX_BIRTH, bootId: bootId.trim() };
  return sameLinuxBirth(expected, actual) && expected.pgid === row.pgid
    ? row.threads
    : "unavailable";
}

function macThreadRows(text) {
  const lines = String(text ?? "")
    .split("\n")
    .filter((line) => line.trim() !== "");
  return lines.length >= 2 ? lines.length - 1 : "unavailable";
}

export function threadCount(
  entry,
  runImpl = run,
  { os = process.platform, read = readFileSync } = {},
) {
  const pid = entry.pid ?? entry;
  return (
    safe(() => {
      if (os !== "linux")
        return macThreadRows(runImpl("ps", ["-M", "-p", String(pid)], probeOptions()));
      const birth = typeof entry === "number" ? linuxBirth(pid) : entry;
      if (!birth) return "unavailable";
      return linuxThreadCount(birth, read(`/proc/${pid}/stat`, "utf8"), read(BOOT_ID_PATH, "utf8"));
    }) ?? "unavailable"
  );
}

export function probeMac(pid = process.pid) {
  const fields = {};
  for (const f of ["rss", "vsz", "lstart", "pgid", "ppid", "comm", "args"])
    fields[f] = probeField(f, pid);
  const threads = threadCount(pid);
  fields.threads =
    threads === "unavailable"
      ? { works: false }
      : {
          works: true,
          example: String(threads),
          method:
            process.platform === "linux"
              ? "/proc/PID/stat num_threads"
              : "ps -M line count minus header",
        };
  return {
    schema: "dev-browser-observe.mac-probe.v1",
    probedAt: isoNow(),
    host: hostIdentity(),
    fields,
    units: { rss: "KiB", vsz: "KiB" },
    labels: SAMPLE_LABELS,
  };
}

export function hostIdentity() {
  const sysctl = (name) => safe(() => run("sysctl", ["-n", name], probeOptions()).trim());
  const translated = sysctl("sysctl.proc_translated");
  return {
    platform: platform(),
    arch: arch(),
    osRelease: release(),
    productVersion: safe(() => run("sw_vers", ["-productVersion"]).trim()),
    hwModel: sysctl("hw.model"),
    cpuBrand: sysctl("machdep.cpu.brand_string"),
    procTranslated: translated === null ? "unavailable" : translated,
    hwMemsizeBytes: Number(sysctl("hw.memsize")) || totalmem(),
    freememBytes: freemem(),
    loadavg: loadavg(),
    nodeVersion: process.version,
  };
}

export function parseSampleRows(text) {
  const rows = [];
  for (const line of text.split("\n")) {
    const m = line.trim().match(/^(\d+)\s+(\d+)\s+(\d+)$/);
    if (m) rows.push({ pid: Number(m[1]), rssKiB: Number(m[2]), vszKiB: Number(m[3]) });
  }
  return rows;
}

/** Instantaneous sample of the given pids; missing fields are recorded as "unavailable". */
export function sample(owned, { runImpl = run, withThreads = true, timing } = {}) {
  const pids = owned.map((entry) => entry.pid ?? entry);
  const at = isoNow();
  const hr = process.hrtime.bigint().toString();
  if (pids.length === 0)
    return { at, hrtimeNs: hr, processes: [], summedRssKiB: 0, label: SAMPLE_LABELS.rss };
  // ps -p exits 1 when any listed pid is gone; its stdout still has the live rows.
  let text = "";
  try {
    text = timed(timing, "RSS ps", { pids: pids.length }, () =>
      runImpl("ps", ["-o", "pid=,rss=,vsz=", "-p", pids.join(",")], probeOptions()),
    );
  } catch (error) {
    text = error?.stdout?.toString?.() ?? "";
  }
  const rows = parseSampleRows(text);
  const processes = pids.map((pid, index) => {
    const r = rows.find((x) => x.pid === pid);
    return {
      pid,
      present: Boolean(r),
      rssKiB: r ? r.rssKiB : "unavailable",
      vszKiB: r ? r.vszKiB : "unavailable",
      threads:
        r && withThreads
          ? timed(timing, "thread reads", { pid }, () => threadCount(owned[index], runImpl))
          : "unavailable",
    };
  });
  const summed = processes.reduce(
    (acc, p) => (typeof p.rssKiB === "number" ? acc + p.rssKiB : acc),
    0,
  );
  return {
    at,
    hrtimeNs: hr,
    processes,
    summedRssKiB: summed,
    label: SAMPLE_LABELS.rss,
    loadavg: loadavg(),
  };
}

export function sampledPeak(samples) {
  let peak = 0;
  for (const s of samples) peak = Math.max(peak, s.summedRssKiB ?? 0);
  return { summedRssKiB: peak, sampleCount: samples.length, label: SAMPLE_LABELS.peak };
}

const execFileAsync = promisify(execFile);

/** Linux reads num_threads and start ticks from the same kernel observation. */
export async function threadCountAsync(
  entry,
  execImpl = execFileAsync,
  { os = process.platform, read = readFile } = {},
) {
  const pid = entry.pid ?? entry;
  try {
    if (os !== "linux")
      return macThreadRows(
        (await execImpl("ps", ["-M", "-p", String(pid)], probeOptions())).stdout,
      );
    const birth = typeof entry === "number" ? linuxBirth(pid) : entry;
    if (!birth) return "unavailable";
    const [stat, bootId] = await Promise.all([
      read(`/proc/${pid}/stat`, "utf8"),
      read(BOOT_ID_PATH, "utf8"),
    ]);
    return linuxThreadCount(birth, stat, bootId);
  } catch {
    return "unavailable";
  }
}

/**
 * Non-blocking variant used during the armed wait, so sampling never stalls
 * the event loop that timestamps binding delivery. Threads are sampled only with
 * withThreads (the resource study), one platform-specific observation per present pid.
 */
export async function sampleAsync(
  owned,
  { withThreads = false, execImpl = execFileAsync, os = process.platform } = {},
) {
  const pids = owned.map((entry) => entry.pid ?? entry);
  const at = isoNow();
  const hr = process.hrtime.bigint().toString();
  if (pids.length === 0)
    return { at, hrtimeNs: hr, processes: [], summedRssKiB: 0, label: SAMPLE_LABELS.rss };
  let text = "";
  try {
    text = (await execImpl("ps", ["-o", "pid=,rss=,vsz=", "-p", pids.join(",")], probeOptions()))
      .stdout;
  } catch (error) {
    text = error?.stdout ?? "";
  }
  const rows = parseSampleRows(text);
  const processes = await Promise.all(
    pids.map(async (pid, index) => {
      const r = rows.find((x) => x.pid === pid);
      return {
        pid,
        present: Boolean(r),
        rssKiB: r ? r.rssKiB : "unavailable",
        vszKiB: r ? r.vszKiB : "unavailable",
        threads:
          r && withThreads ? await threadCountAsync(owned[index], execImpl, { os }) : "unavailable",
      };
    }),
  );
  const summed = processes.reduce(
    (acc, p) => (typeof p.rssKiB === "number" ? acc + p.rssKiB : acc),
    0,
  );
  const threadCounts = processes.map((p) => p.threads).filter((t) => typeof t === "number");
  return {
    at,
    hrtimeNs: hr,
    processes,
    summedRssKiB: summed,
    label: SAMPLE_LABELS.rss,
    ...(withThreads
      ? {
          summedThreads:
            threadCounts.length === processes.filter((p) => p.present).length &&
            threadCounts.length > 0
              ? threadCounts.reduce((a, b) => a + b, 0)
              : "unavailable",
        }
      : { threadsNote: "not sampled during the armed wait" }),
  };
}
