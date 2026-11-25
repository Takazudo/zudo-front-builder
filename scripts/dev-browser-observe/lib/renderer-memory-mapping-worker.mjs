import { createHash } from "node:crypto";
import { mkdir, open, readlink, stat, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { linuxBirth, sameLinuxBirth } from "./linux-birth.mjs";
import {
  RENDERER_MEMORY_MAPPING_BOUNDARIES as boundaries,
  RENDERER_MEMORY_MAPPING_LIMITS as limits,
} from "./renderer-memory-protocol.mjs";

const HEADER =
  /^([0-9a-f]+)-([0-9a-f]+) ([r-][w-][x-][ps]) ([0-9a-f]+) ([0-9a-f]+:[0-9a-f]+) (\d+)(?: +([^\r\n]*))?$/;
const MANDATORY = ["Rss", "Private_Clean", "Private_Dirty"];
const OPTIONAL = ["Pss", "Private_Hugetlb", "Swap"];
const fields = [...MANDATORY, ...OPTIONAL];
const categories = ["fileBacked", "named", "mainHeap", "anonymousUnclassified"];

export function summarizeMappings(text, { smaps }) {
  if (!text.endsWith("\n")) throw new Error("mapping input lacks a complete final line");
  const totals = Object.fromEntries(
    categories.map((category) => [
      category,
      {
        count: 0,
        sizeBytes: 0,
        ...(smaps && { valuesKiB: Object.fromEntries(fields.map((key) => [key, 0])) }),
      },
    ]),
  );
  let row;
  let previousEnd = 0n;
  let count = 0;
  const addRow = () => {
    if (!row) return;
    const group = totals[row.category];
    count += 1;
    group.count += 1;
    group.sizeBytes += row.sizeBytes;
    if (!Number.isSafeInteger(group.sizeBytes))
      throw new Error("mapping sizes exceed safe integer");
    if (!smaps) return;
    for (const key of fields) {
      const value = row.values[key];
      if (value === undefined && MANDATORY.includes(key))
        throw new Error(`smaps row missing ${key}`);
      const total = group.valuesKiB[key];
      if (total === null || value === undefined) group.valuesKiB[key] = null;
      else {
        if (!Number.isSafeInteger(total + value))
          throw new Error(`smaps total exceeds safe integer: ${key}`);
        group.valuesKiB[key] += value;
      }
    }
  };
  for (const line of text.slice(0, -1).split("\n")) {
    const header = HEADER.exec(line);
    if (header) {
      addRow();
      const [, lo, hi, , , , inode, pathname = ""] = header;
      const from = BigInt("0x" + lo);
      const to = BigInt("0x" + hi);
      const sizeBytes = Number(to - from);
      if (from < previousEnd || to <= from || !Number.isSafeInteger(sizeBytes))
        throw new Error("invalid or overlapping mapping range");
      previousEnd = to;
      const category =
        pathname === "[heap]"
          ? "mainHeap"
          : pathname.startsWith("[") && pathname.endsWith("]")
            ? "named"
            : BigInt(inode) !== 0n
              ? "fileBacked"
              : pathname
                ? "named"
                : "anonymousUnclassified";
      row = { category, sizeBytes, values: {} };
      continue;
    }
    const metric = smaps && row && /^(\w+):\s*(\S(?:.*\S)?)?\s*$/.exec(line);
    if (!metric) throw new Error("invalid procfs mapping or smaps metric line");
    const [, key, raw] = metric;
    if (!fields.includes(key)) continue;
    const value = /^(\d+)\s+kB$/.exec(raw ?? "");
    if (!value || !Number.isSafeInteger(Number(value[1])) || Object.hasOwn(row.values, key))
      throw new Error(`invalid or duplicate smaps ${key} value`);
    row.values[key] = Number(value[1]);
  }
  addRow();
  if (!count) throw new Error("no process mappings");
  return { count, categories: totals };
}

async function verifyProcess(expected, executablePath, priorPath = null) {
  const birth = linuxBirth(expected.pid);
  if (!sameLinuxBirth(expected, birth) || birth.pgid !== expected.pgid || birth.state === "Z")
    throw new Error("mapping capture process birth mismatch");
  const exe = `/proc/${birth.pid}/exe`;
  const [path, observed, pinned] = await Promise.all([
    readlink(exe),
    stat(exe, { bigint: true }),
    stat(executablePath, { bigint: true }),
  ]);
  if (
    observed.dev !== pinned.dev ||
    observed.ino !== pinned.ino ||
    (priorPath !== null && path !== priorPath)
  )
    throw new Error("mapping capture executable identity mismatch");
  return {
    pid: birth.pid,
    pgid: birth.pgid,
    bootId: birth.bootId,
    startTicks: birth.startTicks,
    executable: { path, device: String(observed.dev), inode: String(observed.ino) },
  };
}

export async function readMappingInput(path, buffer, start, interval) {
  interval.startHrtimeNs = String(process.hrtime.bigint());
  let file;
  let offset = start;
  try {
    file = await open(path, "r");
    while (offset < buffer.length) {
      const length = Math.min(64 * 1024, buffer.length - offset);
      const { bytesRead } = await file.read(buffer, offset, length, null);
      if (!bytesRead) return buffer.subarray(start, offset);
      offset += bytesRead;
    }
    throw new Error("maps/smaps combined input cap reached");
  } finally {
    interval.endHrtimeNs = String(process.hrtime.bigint());
    interval.bytes = offset - start;
    await file?.close();
  }
}

async function collect({ expected, executablePath, artifactDir, boundary }, evidence) {
  if (!boundaries.includes(boundary)) throw new Error("unknown mapping boundary");
  const directory = join(artifactDir, "mappings");
  const collectorBirth = linuxBirth(process.pid);
  const collectorArtifact = join("mappings", `${boundary}.collector.json`);
  await mkdir(directory, { recursive: true });
  await writeFile(join(artifactDir, collectorArtifact), JSON.stringify(collectorBirth) + "\n", {
    flag: "wx",
  });
  evidence.collectorBirth = collectorBirth;
  evidence.collectorArtifact = collectorArtifact;
  const root = `/proc/${expected.pid}`;
  const buffer = Buffer.allocUnsafe(limits.bytes);
  evidence.before = await verifyProcess(expected, executablePath);
  const maps = await readMappingInput(`${root}/maps`, buffer, 0, evidence.maps);
  evidence.between = await verifyProcess(expected, executablePath, evidence.before.executable.path);
  const smaps = await readMappingInput(`${root}/smaps`, buffer, maps.length, evidence.smaps);
  evidence.after = await verifyProcess(expected, executablePath, evidence.before.executable.path);
  if (
    [evidence.between, evidence.after].some(
      (identity) =>
        identity.executable.device !== evidence.before.executable.device ||
        identity.executable.inode !== evidence.before.executable.inode,
    )
  )
    throw new Error("mapping capture executable inode changed during the pair");
  for (const [key, input] of [
    ["maps", maps],
    ["smaps", smaps],
  ]) {
    const name = join("mappings", `${boundary}.${key}`);
    await writeFile(join(artifactDir, name), input, { flag: "wx" });
    evidence[key].artifact = name;
    evidence[key].sha256 = createHash("sha256").update(input).digest("hex");
    evidence[key].summary = summarizeMappings(input.toString("utf8"), { smaps: key === "smaps" });
  }
  return evidence;
}

if (process.argv[1] && pathToFileURL(process.argv[1]).href === import.meta.url) {
  const args = JSON.parse(process.argv[2]);
  const deadline = BigInt(args.deadlineNs);
  const duration = Math.min(limits.timeMs, Number(deadline - process.hrtime.bigint()) / 1e6);
  const timer = setTimeout(() => process.exit(1), Math.max(0, duration));
  const parentGone = () => process.exit(1);
  process.once("disconnect", parentGone);
  const evidence = { maps: {}, smaps: {} };
  let message;
  try {
    if (duration <= 0) throw new Error("mapping capture deadline reached before collection");
    message = { result: await collect(args, evidence) };
  } catch (error) {
    message = { error: String(error?.message ?? error), evidence };
    process.exitCode = 1;
  }
  process.send(message, () => {
    clearTimeout(timer);
    process.off("disconnect", parentGone);
    if (process.connected) process.disconnect();
  });
}
