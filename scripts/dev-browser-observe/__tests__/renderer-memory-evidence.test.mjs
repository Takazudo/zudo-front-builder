import assert from "node:assert/strict";
import { test } from "vitest";
import { inspectRendererLifecycle, parseMemoryKiB } from "../lib/renderer-memory-evidence.mjs";

const heap = {
  used_heap_size: 41,
  total_heap_size: 56,
  total_physical_size: 55,
  external_memory: 121,
  malloced_memory: 10,
  peak_malloced_memory: 20,
  total_allocated_bytes: 71,
  total_global_handles_size: 16,
  used_global_handles_size: 8,
  number_of_native_contexts: 1,
  number_of_detached_contexts: 0,
};

function stream({ omit, failure, lost = 0, adoptions = 1, change = () => {} } = {}) {
  const events = [
    {
      ev: "renderer_lifecycle_start",
      schema_version: 1,
      buffer_capacity: 128,
      drain_capacity: 128,
      attempt_limit: 4096,
      line_limit_bytes: 1024,
      pid: 20,
      clock_monotonic_us: 100000,
      anchor_before_us: 0,
      anchor_after_us: 10,
    },
  ];
  let occurrence = 0;
  for (let host = 1; host <= adoptions; host++) {
    const identity = { refresh_id: host, snapshot_gen: host };
    const owner = { tid: 30 + host, tid_start_ticks: 12 + host };
    const phaseRows = [
      ["refresh_start", identity],
      ["snapshot_ready", identity],
      ["bundle_ready", identity],
      ["host_create"],
      ["host_initialized", owner],
      ["host_boot", { ...owner, heap }],
      ["validation_ok"],
      ["adopted", { worker_gen: host }],
      ["retire_requested"],
      ["host_final_heap", { ...owner, heap }],
      ["host_destroyed", owner],
      [failure ?? "join_completed"],
    ];
    for (const [phase, fields] of phaseRows) {
      if (
        host < adoptions &&
        ["retire_requested", "host_final_heap", "host_destroyed", "join_completed"].includes(phase)
      ) {
        // Emitted only after adoption of the next host, with no identity inferred from delivery order.
        continue;
      }

      occurrence++;
      if (phase === omit) continue;
      const record = {
        ev: "renderer_lifecycle",
        ...identity,
        host_id: host,
        occurred_us: occurrence,
        occurrence,
        phase,
        ...fields,
      };
      change(record);
      events.push(record);
      if (phase === "adopted" && host > 1) {
        const previous = host - 1;
        for (const previousPhase of [
          "retire_requested",
          "host_final_heap",
          "host_destroyed",
          "join_completed",
        ]) {
          events.push({
            ev: "renderer_lifecycle",
            occurred_us: ++occurrence,
            occurrence,
            phase: previousPhase,
            refresh_id: previous,
            snapshot_gen: previous,
            host_id: previous,
            ...(["retire_requested", "join_completed"].includes(previousPhase)
              ? { retired_by_refresh_id: host }
              : { tid: 30 + previous, tid_start_ticks: 12 + previous }),
            ...(previousPhase === "host_final_heap" ? { heap } : {}),
          });
        }
      }
    }
  }
  events.push({
    ev: "renderer_lifecycle_terminal",
    attempted: occurrence,
    emitted: occurrence,
    contention: lost,
    overflow: 0,
    output_dropped: 0,
  });
  // Occurrence and pipe emission have separate orderings.
  [events[3], events[4]] = [events[4], events[3]];
  return {
    handle: { native: { pid: 20 } },
    provSince: () => ({
      droppedBeforeMark: 0,
      lines: events.map((entry, index) => ({
        ns: BigInt(index),
        text: "[zfb-prov] " + JSON.stringify({ seq: index + 1, ms: index, ...entry }),
      })),
    }),
  };
}

test("decodes the proc memory domains without treating unavailable as zero", () => {
  assert.deepEqual(
    parseMemoryKiB("VmRSS: 90 kB\nRssFile:\t0 kB\nVmSwap: unknown\n", [
      "VmRSS",
      "RssFile",
      "RssAnon",
      "VmSwap",
    ]),
    { VmRSS: 90, RssFile: 0, RssAnon: null, VmSwap: null },
  );
});

test("accepts complete retirement evidence in occurrence order and verifies adoption count", () => {
  const report = inspectRendererLifecycle(stream({ adoptions: 2 }), {
    final: true,
    minimumAdoptions: 2,
  });
  assert.equal(report.failure, null);
  assert.equal(report.complete, true);
  assert.equal(report.adoptions, 2);
  const short = inspectRendererLifecycle(stream(), { final: true, minimumAdoptions: 2 });
  assert.equal(short.failure.reason, "renderer-telemetry");
});

test("fails closed on counted loss, an absent tail, or a foreign owner birth", () => {
  const cases = [
    stream({ lost: 1 }),
    stream({ omit: "host_destroyed" }),
    stream({
      change: (record) => {
        if (record.phase === "host_final_heap") record.tid_start_ticks += 1;
      },
    }),
  ];
  const noTail = cases[0].provSince();
  cases.push({ provSince: () => ({ ...noTail, lines: noTail.lines.slice(0, -1) }) });
  for (const server of cases) {
    const report = inspectRendererLifecycle(server, { final: true });
    assert.equal(report.failure.reason, "renderer-telemetry");
    assert.equal(report.complete, false);
  }
});

test("a recorded timeout can establish an observed join failure without claiming disposal", () => {
  const server = stream({ omit: "host_destroyed", failure: "join_timeout" });
  // Account for the deliberate absence of a destruction record during the timeout.
  const { lines, droppedBeforeMark } = server.provSince();
  const parsed = lines.map(({ text }) => JSON.parse(text.slice("[zfb-prov] ".length)));
  parsed.find((entry) => entry.phase === "join_timeout").occurrence--;
  parsed.at(-1).attempted--;
  parsed.at(-1).emitted--;
  const report = inspectRendererLifecycle(
    {
      ...server,
      provSince: () => ({
        droppedBeforeMark,
        lines: parsed.map((entry) => ({ text: "[zfb-prov] " + JSON.stringify(entry) })),
      }),
    },
    { final: true },
  );
  assert.equal(report.failure.reason, "renderer-native-failure");
  assert.equal(report.nativeFailures[0].phase, "join_timeout");
  assert.equal(report.complete, true);
});
