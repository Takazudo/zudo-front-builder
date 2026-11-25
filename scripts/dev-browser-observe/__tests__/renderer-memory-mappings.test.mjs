import assert from "node:assert/strict";
import { EventEmitter } from "node:events";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "vitest";
import { linuxBirth } from "../lib/linux-birth.mjs";
import { readMappingInput, summarizeMappings } from "../lib/renderer-memory-mapping-worker.mjs";
import { createMappingCollector } from "../lib/renderer-memory-mappings.mjs";

const limits = { timeoutMs: 2000, signal: new AbortController().signal };
const entries = [
  "1000-2000 r--p 00000000 00:63 4     /tmp/some file (deleted)",
  "3000-4000 rw-p 00000000 00:00 0     [heap]",
  "4000-5000 rw-s 00000000 00:00 13     [anon_shmem:named region]",
  "7000-8000 rw-p 00000000 00:00 0",
];

test("classifies address ranges by mapping identity and associates smaps rows", () => {
  const maps = summarizeMappings(entries.join("\n") + "\n", { smaps: false });
  assert.equal(maps.count, 4);
  for (const row of Object.values(maps.categories))
    assert.deepEqual(row, { count: 1, sizeBytes: 4096 });
  const text = entries
    .map((entry) => entry + "\nRss: 4 kB\nPrivate_Clean: 0 kB\nPrivate_Dirty: 3 kB\n")
    .join("");
  const smaps = summarizeMappings(text, { smaps: true });
  assert.deepEqual(smaps.categories.anonymousUnclassified.valuesKiB, {
    Rss: 4,
    Private_Clean: 0,
    Private_Dirty: 3,
    Pss: null,
    Private_Hugetlb: null,
    Swap: null,
  });
  assert.throws(
    () => summarizeMappings(text.replace("Rss: 4 kB\n", ""), { smaps: true }),
    /missing Rss/,
  );
  assert.throws(() => summarizeMappings(text.trimEnd(), { smaps: true }), /complete final line/);
  assert.throws(
    () => summarizeMappings(text.replace("4000-5000", "3500-5000"), { smaps: true }),
    /overlapping/,
  );
});

test("enforces the shared cap at the read boundary before consuming the rest of the input", async () => {
  const dir = await mkdtemp(join(tmpdir(), "zfb-mapping-cap-"));
  try {
    const path = join(dir, "input");
    await writeFile(path, "0123456789");
    const target = Buffer.alloc(6);
    const interval = {};
    await assert.rejects(readMappingInput(path, target, 2, interval), /combined input cap reached/);
    assert.equal(interval.bytes, 4);
    assert.equal(target.subarray(2).toString(), "0123");
    assert(BigInt(interval.endHrtimeNs) >= BigInt(interval.startHrtimeNs));
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("times out a stalled direct helper and accounts for its separate close", async () => {
  class FakeChild extends EventEmitter {
    pid = 71;
    exitCode = null;
    signalCode = null;
    killed = 0;
    kill(signal) {
      assert.equal(signal, "SIGKILL");
      this.killed += 1;
      this.signalCode = signal;
    }
  }
  const child = new FakeChild();
  const collector = createMappingCollector({ launch: () => child });
  await assert.rejects(collector.collect({ ...limits, timeoutMs: 10 }), /deadline reached/);
  assert.equal(child.killed, 1);
  assert.equal(collector.activePid, 71);
  const closed = collector.stop();
  child.emit("close", null, "SIGKILL");
  await closed;
  assert.equal(collector.activePid, null);
  assert.equal(child.killed, 1);
});

test.skipIf(process.platform !== "linux")(
  "captures a real pair with raw files and rejects a stale birth",
  async () => {
    const collector = createMappingCollector();
    const dir = await mkdtemp(join(tmpdir(), "zfb-mapping-native-"));
    const expected = linuxBirth(process.pid);
    const args = {
      expected,
      executablePath: process.execPath,
      artifactDir: dir,
      boundary: "before-edits",
      ...limits,
    };
    try {
      const result = await collector.collect(args);
      assert.equal(result.collectorClosed, true);
      assert.equal(result.collectorBirth.pid, result.collectorPid);
      assert.deepEqual(
        JSON.parse(await readFile(join(dir, result.collectorArtifact), "utf8")),
        result.collectorBirth,
      );
      assert.equal(collector.activePid, null);
      assert.deepEqual(result.before, result.between);
      assert.deepEqual(result.between, result.after);
      assert.equal(result.after.pid, process.pid);
      for (const key of ["maps", "smaps"]) {
        assert.equal((await readFile(join(dir, result[key].artifact))).length, result[key].bytes);
        assert(result[key].summary.count > 0);
      }
      assert(BigInt(result.maps.endHrtimeNs) <= BigInt(result.smaps.startHrtimeNs));
      await assert.rejects(
        collector.collect({
          ...args,
          expected: { ...expected, startTicks: "0" },
          boundary: "after-edits",
        }),
        /process birth mismatch/,
      );
      assert.equal(collector.activePid, null);
      await assert.rejects(
        collector.collect({
          ...args,
          executablePath: "/usr/bin/false",
          boundary: "after-route-verification",
        }),
        /executable identity mismatch/,
      );
    } finally {
      await collector.stop();
      await rm(dir, { recursive: true, force: true });
    }
  },
);
