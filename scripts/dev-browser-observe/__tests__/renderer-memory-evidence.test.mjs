import assert from "node:assert/strict";
import { test } from "vitest";
import { inspectRendererLifecycle, parseMemoryKiB } from "../lib/renderer-memory-evidence.mjs";
import { parseReclamationRollup } from "../lib/renderer-memory-reclamation.mjs";

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

function allocatorStream({ unavailable, unfinished, omit, corrupt = () => {} } = {}) {
  const original = stream()
    .provSince()
    .lines.map(({ text }) => JSON.parse(text.slice(11)));
  const [anchor, ...rest] = original;
  const rows = [
    { ...anchor, allocator_metric_domain: "gnu_mallinfo2.v1", allocator_call_limit: 128 },
  ];
  let call = 0;
  let occurrence = 0;
  const append = (row) => {
    occurrence += 1;
    rows.push({ tick: 23, ...row, occurrence, occurred_us: occurrence });
  };
  const getter = (allocator_phase, allocator_boundary_outcome, hasSnapshot = true) => {
    if (allocator_phase === omit) return;
    const identity = {
      ev: "renderer_lifecycle",
      metric_domain: "gnu_mallinfo2.v1",
      refresh_id: 1,
      ...(hasSnapshot && { snapshot_gen: 1 }),
      ...(allocator_phase === "refresh_return" && { host_id: 1 }),
      allocator_phase,
      allocator_call: ++call,
      allocator_boundary_outcome,
    };
    const absent = unavailable === allocator_phase;
    if (!absent) append({ ...identity, phase: "allocator_start" });
    if (unfinished === allocator_phase) return;
    const completed = occurrence + 1;
    append({
      ...identity,
      phase: "allocator_complete",
      allocator_started_us: absent ? completed : completed - 1,
      allocator_completed_us: completed,
      allocator_duration_us: absent ? 0 : 1,
      allocator_outcome: absent ? "unavailable" : "ok",
      ...(absent
        ? { allocator_unavailable_reason: "application_write_exclusion" }
        : {
            allocator: {
              arena_bytes: 171,
              uordblks_bytes: 11,
              fordblks_bytes: 160,
              hblkhd_bytes: 62,
              hblks: 2,
              fsmblks_bytes: 0,
            },
          }),
    });
  };
  const phase = (name) => ({
    ev: "renderer_lifecycle",
    phase: name,
    refresh_id: 1,
    snapshot_gen: 1,
  });
  append({ ...phase("refresh_start"), boot: false });
  getter("entry", undefined, false);
  append(phase("snapshot_start"));
  getter("snapshot_before", undefined, false);
  append(phase("snapshot_ready"));
  getter("snapshot_after", "ok");
  append(phase("bundle_start"));
  getter("bundle_before");
  getter("bundle_after", "ok");
  append(phase("bundle_ready"));
  const byOccurrence = rest.sort((a, b) => a.occurrence - b.occurrence);
  for (const name of ["host_create", "host_initialized", "host_boot", "validation_ok", "adopted"])
    append(byOccurrence.find((row) => row.phase === name));
  append({ ...phase("refresh_complete"), host_id: 1 });
  getter("refresh_return", "refreshed");
  for (const name of ["retire_requested", "host_final_heap", "host_destroyed", "join_completed"])
    append(byOccurrence.find((row) => row.phase === name));
  if (!unfinished)
    rows.push({
      ev: "renderer_lifecycle_terminal",
      attempted: occurrence,
      emitted: occurrence,
      contention: 0,
      overflow: 0,
      output_dropped: 0,
    });
  for (const row of rows) corrupt(row);
  return {
    handle: { native: { pid: 20 } },
    provSince: () => ({
      droppedBeforeMark: 0,
      lines: rows.map((row, i) => ({
        text: "[zfb-prov] " + JSON.stringify({ ...row, seq: i + 1, ms: i }),
      })),
    }),
  };
}

test("validates v2 GNU checkpoint units, clock, occurrence and explicit post-return phase", () => {
  const report = inspectRendererLifecycle(allocatorStream(), { final: true, withAllocator: true });
  assert.equal(report.complete, true, JSON.stringify(report.failure));
  assert.equal(report.failure, null);
  assert.deepEqual(report.allocator, {
    domain: "gnu_mallinfo2.v1",
    limit: 128,
    started: 6,
    completed: 6,
    unfinished: [],
    unavailable: [],
    endIdle: "unavailable: no end-idle GNU allocator call",
    failure: null,
  });
  for (const server of [
    allocatorStream({ omit: "refresh_return" }),
    allocatorStream({
      corrupt(row) {
        if (row.phase === "allocator_complete" && row.allocator_phase === "refresh_return")
          row.allocator_duration_us += 1;
      },
    }),
    allocatorStream({
      corrupt(row) {
        if (row.phase === "allocator_complete" && row.allocator_phase === "entry")
          row.allocator.arena_bytes = null;
      },
    }),
    stream(),
  ]) {
    const invalid = inspectRendererLifecycle(server, { final: true, withAllocator: true });
    assert.equal(invalid.complete, false);
    assert.equal(invalid.failure.reason, "renderer-telemetry");
  }
});

test("reports in-flight and unfinished GNU getters independently of native join outcomes", () => {
  const server = allocatorStream({ unfinished: "refresh_return" });
  const ongoing = inspectRendererLifecycle(server, { withAllocator: true });
  assert.equal(ongoing.failure, null);
  assert.deepEqual(
    ongoing.allocator.unfinished.map((entry) => entry.phase),
    ["refresh_return"],
  );
  const final = inspectRendererLifecycle(server, { final: true, withAllocator: true });
  assert.equal(final.complete, false);
  assert.match(final.failure.detail, /started but did not complete/);
  assert.deepEqual(final.nativeFailures, []);
});

test("makes an explicit application lock exclusion incomplete without inventing a getter start", () => {
  const report = inspectRendererLifecycle(allocatorStream({ unavailable: "refresh_return" }), {
    final: true,
    withAllocator: true,
  });
  assert.equal(report.complete, false);
  assert.match(report.failure.detail, /application_write_exclusion/);
  assert.equal(report.lifecycleComplete, true);
  assert.equal(report.allocator.started, 5);
  assert.equal(report.allocator.completed, 5);
  assert.deepEqual(report.allocator.unfinished, []);
  assert.deepEqual(report.allocator.unavailable, [
    { call: 6, refreshId: 1, phase: "refresh_return", reason: "application_write_exclusion" },
  ]);
});

test("rejects mismatched completion hosts and foreign refresh correlation", () => {
  for (const mutate of [
    (row) => {
      if (row.allocator_phase === "refresh_return" && row.phase === "allocator_complete")
        row.host_id = 81;
    },
    (row) => {
      if (row.allocator_phase === "refresh_return") row.host_id = 81;
    },
    (row) => {
      if (row.allocator_phase === "entry") row.tick = 85;
    },
    (row) => {
      if (row.allocator_phase === "entry") row.snapshot_gen = 82;
    },
  ]) {
    const report = inspectRendererLifecycle(allocatorStream({ corrupt: mutate }), {
      final: true,
      withAllocator: true,
    });
    assert.equal(report.complete, false);
    assert.match(report.failure.detail, /matching start|foreign tick, host or snapshot/);
  }
});

test("rejects allocator completion fields on start and fabricated counters on unavailable", () => {
  for (const mutate of [
    (row) => {
      if (row.phase === "allocator_start") row.allocator_outcome = "unavailable";
    },
    (row) => {
      if (row.allocator_outcome === "unavailable") row.allocator = {};
    },
  ]) {
    const report = inspectRendererLifecycle(
      allocatorStream({ unavailable: "refresh_return", corrupt: mutate }),
      { final: true, withAllocator: true },
    );
    assert.equal(report.complete, false);
    assert.match(report.failure.detail, /completion fields|invalid unavailable/);
  }
});

test("checks a closed refresh in the contiguous prefix and tolerates reordered emission", () => {
  const missing = inspectRendererLifecycle(allocatorStream({ omit: "snapshot_after" }), {
    withAllocator: true,
  });
  assert.match(missing.failure.detail, /schedule incomplete/);

  const server = allocatorStream();
  const rows = server.provSince().lines.map(({ text }) => JSON.parse(text.slice(11)));
  const index = rows.findIndex((row) => row.allocator_phase === "snapshot_after");
  // Temporarily unseen first half of the pair, not unexplained loss: it will be
  // emitted after the closed refresh marker, while its occurrence stays earlier.
  const delayed = rows.splice(index, 1)[0];
  const inspect = (rows, final) =>
    inspectRendererLifecycle(
      {
        ...server,
        provSince: () => ({
          droppedBeforeMark: 0,
          lines: rows.map((row, i) => ({
            text: "[zfb-prov] " + JSON.stringify({ ...row, seq: i + 1 }),
          })),
        }),
      },
      { withAllocator: true, final },
    );
  assert.equal(inspect(rows.slice(0, -1), false).failure, null);
  rows.splice(-1, 0, delayed);
  assert.equal(inspect(rows, true).failure, null);
  const watermark = rows.pop();
  rows.splice(-1, 0, watermark);
  assert.match(inspect(rows, true).failure.detail, /terminal watermark/);
});

function postApplyStream({ omitLastGetter = false, change = () => {} } = {}) {
  const source = stream({ adoptions: 14 })
    .provSince()
    .lines.map(({ text }) => JSON.parse(text.slice(11)));
  const rows = [
    {
      ...source[0],
      allocator_metric_domain: "gnu_mallinfo2.v1",
      allocator_call_limit: 128,
      allocator_post_apply: true,
    },
  ];
  const path = "/corpus/article-00005.md";
  const mutations = [];
  let occurrence = 0;
  let calls = 0;
  let relativeUs = 0;
  const append = (row) =>
    rows.push({
      ...row,
      ev: "renderer_lifecycle",
      occurrence: ++occurrence,
      occurred_us: ++relativeUs,
    });
  for (let host = 1; host <= 14; host++) {
    const boot = host === 1;
    const tick = boot ? undefined : host - 1;
    relativeUs = host * 100_000;
    if (!boot) {
      rows.push({ ev: "intake", ms: relativeUs / 1000, tick, paths: [{ path, kind: "Modified" }] });
      mutations.push({
        kind: host === 14 ? "restoration" : "edit",
        ...(host !== 14 && { edit: tick }),
        paths: [path],
        t0Ns: String((relativeUs + 95_000) * 1000),
        witnessNs: String((relativeUs + 110_000) * 1000),
      });
    }
    const base = { refresh_id: host, tick };
    const snapshot = { ...base, snapshot_gen: host };
    const identity = { ...snapshot, host_id: host };
    const counters = {
      arena_bytes: 170 + host,
      uordblks_bytes: 10,
      fordblks_bytes: 160 + host,
      hblkhd_bytes: 62,
      hblks: 2,
      fsmblks_bytes: 0,
    };
    const getter = (name, boundary, context, { locked = !boot, fields = {} } = {}) => {
      const row = {
        ...context,
        ...fields,
        metric_domain: "gnu_mallinfo2.v1",
        allocator_phase: name,
        allocator_call: ++calls,
        allocator_boundary_outcome: boundary,
      };
      if (!locked) append({ ...row, phase: "allocator_start" });
      append({
        ...row,
        phase: "allocator_complete",
        allocator_started_us: relativeUs + (locked ? 1 : 0),
        allocator_completed_us: relativeUs + 1,
        allocator_duration_us: locked ? 0 : 1,
        allocator_outcome: locked ? "unavailable" : "ok",
        ...(locked
          ? { allocator_unavailable_reason: "application_write_exclusion" }
          : { allocator: counters }),
      });
    };
    const hostRow = (id, phase) => {
      const original = source.find((row) => row.host_id === id && row.phase === phase);
      append({ ...original, tick: id === 1 ? undefined : id - 1 });
    };
    append({ ...base, phase: "refresh_start", boot });
    getter("entry", undefined, base);
    append({ ...base, phase: "snapshot_start" });
    getter("snapshot_before", undefined, base);
    append({ ...snapshot, phase: "snapshot_ready" });
    getter("snapshot_after", "ok", snapshot);
    append({ ...snapshot, phase: "bundle_start" });
    getter("bundle_before", undefined, snapshot);
    getter("bundle_after", "ok", snapshot);
    append({ ...snapshot, phase: "bundle_ready" });
    for (const phase of [
      "host_create",
      "host_initialized",
      "host_boot",
      "validation_ok",
      "adopted",
    ])
      hostRow(host, phase);
    if (!boot) {
      for (const phase of [
        "retire_requested",
        "host_final_heap",
        "host_destroyed",
        "join_completed",
      ])
        hostRow(host - 1, phase);
      getter("retirement_after", "returned", identity);
    }
    append({ ...identity, phase: "refresh_complete" });
    if (!boot) {
      getter("refresh_return", "refreshed", identity);
      const fields = { apply_id: host, apply_outcome: "ok", refresh_outcome: "refreshed" };
      append({ ...identity, ...fields, phase: "apply_return", apply_association: "associated" });
      if (host !== 14 || !omitLastGetter)
        getter("post_apply_return", "returned", identity, { locked: false, fields });
    } else {
      append({
        refresh_id: 0,
        phase: "apply_return",
        apply_id: host,
        apply_outcome: "ok",
        apply_association: "no_refresh",
      });
    }
    if (host === 14)
      for (const phase of [
        "retire_requested",
        "host_final_heap",
        "host_destroyed",
        "join_completed",
      ])
        hostRow(host, phase);
  }
  rows.push({
    ev: "renderer_lifecycle_terminal",
    attempted: occurrence,
    emitted: occurrence,
    contention: 0,
    overflow: 0,
    output_dropped: 0,
  });
  change(rows, mutations);
  return {
    server: {
      handle: { native: { pid: 20 } },
      provSince: () => ({
        droppedBeforeMark: 0,
        lines: rows.map((row, i) => ({
          // Arrival is deliberately after every browser witness.
          ns: "9999999999",
          text: "[zfb-prov] " + JSON.stringify({ ...row, seq: i + 1, ms: row.ms ?? i }),
        })),
      }),
    },
    mutations,
  };
}

function inspectPostApplyStream(options) {
  const { server, mutations } = postApplyStream(options);
  return inspectRendererLifecycle(server, {
    final: true,
    withAllocator: true,
    withPostApply: true,
    requireCompletePostApply: true,
    mutations,
  });
}

test("post-apply uses 13 attributed endpoints and eleven same-boundary body transitions", () => {
  const report = inspectPostApplyStream();
  assert.equal(report.failure, null);
  assert.equal(report.complete, true);
  assert.equal(report.allocator.started, 18);
  assert.equal(report.allocator.unavailable.length, 91);
  assert.equal(report.postApply.endpoints.length, 13);
  assert.equal(report.postApply.transitions.length, 11);
  assert.equal(report.postApply.transitions.at(-1).toApply, 13);
  assert.equal(report.postApply.transitions.at(-1).delta.fordblks_bytes, 1);
  assert.equal(report.postApply.restoration.applyId, 14);
  assert.equal(report.postApply.restoration.mutation.kind, "restoration");
  assert.match(report.allocator.endIdle, /unavailable/);
});

test("v2 rejects a post-apply startup contract", () => {
  const { server } = postApplyStream();
  const report = inspectRendererLifecycle(server, { final: true, withAllocator: true });
  assert.equal(report.failure.reason, "renderer-telemetry");
});

const firstApply = (rows) => rows.find((row) => row.apply_id === 2 && row.phase === "apply_return");
const firstGetter = (rows) =>
  rows.find((row) => row.apply_id === 2 && row.phase === "allocator_complete");
const firstIntake = (rows) => rows.find((row) => row.ev === "intake");

test.each([
  [
    "wrong-snapshot apply",
    (rows) => {
      firstApply(rows).snapshot_gen++;
    },
    /inner refresh return/,
  ],
  [
    "duplicate apply identity",
    (rows) => {
      firstApply(rows).apply_id = 1;
    },
    /duplicate post-apply return/,
  ],
  [
    "foreign token",
    (rows) => {
      firstApply(rows).apply_association = "foreign";
    },
    /invalid post-apply association/,
  ],
  [
    "getter after an error",
    (rows) => {
      firstApply(rows).apply_outcome = "error";
    },
    /matching successful apply return/,
  ],
  [
    "foreign completion",
    (rows) => {
      firstGetter(rows).apply_id++;
    },
    /completion lacks its matching start/,
  ],
  [
    "unrelated getter",
    (_rows, mutations) => {
      mutations.shift();
    },
    /unrelated post-apply observation/,
  ],
  [
    "coalesced intake",
    (rows) => {
      firstIntake(rows).paths.push({ path: "/unrelated.css", kind: "Modified" });
    },
    /ambiguous or coalesced native intake/,
  ],
  [
    "intake within anchor rounding of the write",
    (_rows, mutations) => {
      // The anchor puts intake's earliest possible timestamp at 299989000.
      // The elapsed-us floor makes this intake overlap rather than follow t0.
      mutations[0].t0Ns = "299989500";
    },
    /ambiguous or coalesced native intake/,
  ],
  [
    "next write within endpoint rounding",
    (rows, mutations) => {
      // Preserve a real DOM witness before a getter that finishes later.
      const completion = firstGetter(rows);
      completion.allocator_completed_us += 20_000;
      completion.allocator_duration_us += 20_000;
      completion.occurred_us += 20_000;
      const floorUs = rows[0].clock_monotonic_us + completion.allocator_completed_us;
      mutations[1].t0Ns = String(floorUs * 1000 + 1500);
    },
    /mutation windows overlap/,
  ],
  [
    "native refresh clock before intake",
    (rows) => {
      const intake = firstIntake(rows);
      const refresh = rows.find((row) => row.tick === intake.tick && row.phase === "refresh_start");
      refresh.occurred_us = intake.ms * 1000 - 1;
    },
    /unique successful apply for its intake/,
  ],
  [
    "unknown allocation provider",
    (rows) => {
      const phase = rows.find((row) => row.allocator_unavailable_reason);
      phase.allocator_unavailable_reason = "unsupported_target";
    },
    /GNU allocator getter unavailable/,
  ],
  [
    "loss",
    (rows) => {
      rows.at(-1).contention++;
    },
    /reported telemetry loss/,
  ],
  [
    "an invalid terminal watermark",
    (rows) => {
      rows.at(-1).attempted++;
    },
    /terminal watermark/,
  ],
])("post-apply rejects %s", (_name, change, detail) => {
  const report = inspectPostApplyStream({ change });
  assert.equal(report.complete, false);
  assert.match(report.failure.detail, detail);
});

test("post-apply requires the final successful apply's completed getter", () => {
  const report = inspectPostApplyStream({ omitLastGetter: true });
  assert.equal(report.complete, false);
  assert.match(report.failure.detail, /missing its completed getter/);
});

function reclamationStream({ arm = "trim", trimReturn = 1, change = () => {} } = {}) {
  return postApplyStream({
    change(rows, mutations) {
      Object.assign(rows[0], {
        allocator_reclamation: arm,
        reclamation_target: 13,
        reclamation_rollup_bytes: 16384,
        reclamation_read_limit: 8,
        reclamation_bracket_us: 2_000_000,
        reclamation_chunk_bytes: 128,
      });
      const index = rows.findLastIndex((row) => row.allocator_phase === "post_apply_return");
      const pre = rows[index];
      const identity = Object.fromEntries(
        ["apply_id", "refresh_id", "tick", "snapshot_gen", "host_id"].map((field) => [
          field,
          pre[field],
        ]),
      );
      const extra = [];
      let time = pre.occurred_us;
      const append = (phase, reclamation) => {
        const row = {
          ...identity,
          ev: "renderer_lifecycle",
          phase,
          occurred_us: ++time,
          reclamation,
        };
        extra.push(row);
        return row;
      };
      const rollup = (side, rss) => {
        const text = `1000-2000 ---p 00000000 00:00 0 [rollup]\nRss: ${rss} kB\nAnonymous: ${rss - 10} kB\nPrivate_Clean: 2 kB\nPrivate_Dirty: ${rss - 20} kB\nSwap: 0 kB\nAnonHugePages: 0 kB\n`;
        const bytes = Buffer.from(text);
        const started_us = time + 1;
        append("reclamation_rollup_start", { side, started_us });
        append("reclamation_rollup_complete", {
          side,
          started_us,
          completed_us: time + 1,
          duration_us: 1,
          bytes: bytes.length,
          reads: 2,
          eof: true,
          outcome: "ok",
          rollup: {
            rss_kib: rss,
            anonymous_kib: rss - 10,
            private_clean_kib: 2,
            private_dirty_kib: rss - 20,
            swap_kib: 0,
            anon_huge_pages_kib: 0,
          },
        });
        return () => {
          for (let offset = 0; offset < bytes.length; offset += 128)
            append("reclamation_rollup_chunk", {
              side,
              offset,
              hex: bytes.subarray(offset, offset + 128).toString("hex"),
            });
        };
      };
      rollup("before", 300000)();
      const started_us = time + 1;
      append("reclamation_action_start", { arm, eligible: 13, started_us });
      const performed = arm === "trim";
      append("reclamation_action_complete", {
        arm,
        eligible: 13,
        started_us,
        completed_us: time + 1,
        duration_us: 1,
        performed,
        trim_return: performed ? trimReturn : null,
      });
      const chunks = rollup("after", performed ? 100000 : 300000);
      extra.push({
        ...pre,
        phase: "allocator_start",
        allocator_phase: "reclamation_after",
        allocator_call: pre.allocator_call + 1,
        occurred_us: ++time,
        allocator_started_us: undefined,
        allocator_completed_us: undefined,
        allocator_duration_us: undefined,
        allocator_outcome: undefined,
        allocator: undefined,
      });
      extra.push({
        ...pre,
        allocator_phase: "reclamation_after",
        allocator_call: pre.allocator_call + 1,
        occurred_us: ++time,
        allocator_started_us: time - 1,
        allocator_completed_us: time,
        allocator_duration_us: 1,
      });
      chunks();
      append("reclamation_complete", {
        arm,
        eligible: 13,
        started_us: pre.allocator_started_us,
        completed_us: time + 1,
        duration_us: time + 1 - pre.allocator_started_us,
        performed,
        outcome: "ok",
      });
      for (const row of rows.slice(index + 1))
        if (row.ev === "renderer_lifecycle") row.occurred_us = ++time;
      rows.splice(index + 1, 0, ...extra);
      let occurrence = 0;
      for (const row of rows) if (row.ev === "renderer_lifecycle") row.occurrence = ++occurrence;
      Object.assign(rows.at(-1), { attempted: occurrence, emitted: occurrence });
      change(rows, mutations);
    },
  });
}

function inspectReclamationStream({
  change,
  arm = "trim",
  trimReturn = 1,
  restorationWitness = true,
} = {}) {
  const { server, mutations } = reclamationStream({ arm, trimReturn, change });
  return inspectRendererLifecycle(server, {
    final: true,
    withAllocator: true,
    withPostApply: true,
    reclamationArm: arm,
    restorationWitness,
    mutations,
    requireCompletePostApply: true,
  });
}

test.each(["sham", "trim"])(
  "reclamation validates %s with full native and restoration identities",
  (arm) => {
    const report = inspectReclamationStream({ arm });
    assert.equal(report.failure, null);
    assert.equal(report.complete, true);
    assert.equal(report.postApply.endpoints.length, 13);
    assert.equal(report.reclamation.targetValid, true);
    assert.equal(report.reclamation.complete, true);
    assert.equal(report.reclamation.performed, arm === "trim");
    assert.equal(report.reclamation.deltaKiB.rss_kib, arm === "trim" ? -200000 : 0);
    assert.equal(report.allocator.completed, 19); // The test fixture has eager boot's five getters.
  },
);

test("a zero trim return remains complete evidence", () => {
  const report = inspectReclamationStream({ trimReturn: 0 });
  assert.equal(report.failure, null);
  assert.equal(report.reclamation.action.trim_return, 0);
});

test("reclamation requires a fresh original-body witness after its bracket", () => {
  const report = inspectReclamationStream({ restorationWitness: false });
  assert.match(report.failure.detail, /restoration attribution/);
  assert.equal(report.reclamation.performed, true);
  assert.equal(report.reclamation.targetValid, false);
});

const reclamationRow = (rows, phase, side) =>
  rows.find((row) => row.phase === phase && (!side || row.reclamation.side === side));

test.each([
  [
    "wrong target",
    (rows) => {
      reclamationRow(rows, "reclamation_action_complete").reclamation.eligible = 12;
    },
    /wrong reclamation arm or eligible/,
  ],
  [
    "foreign action",
    (rows) => {
      reclamationRow(rows, "reclamation_action_complete").apply_id++;
    },
    /foreign or duplicate/,
  ],
  [
    "duplicate action",
    (rows) => {
      rows.splice(-1, 0, {
        ...reclamationRow(rows, "reclamation_action_complete"),
        occurrence: rows.at(-1).attempted + 1,
      });
    },
    /duplicate one-shot/,
  ],
  [
    "wrong trim count",
    (rows) => {
      reclamationRow(rows, "reclamation_action_complete").reclamation.performed = false;
    },
    /trim count/,
  ],
  [
    "bad pre-read",
    (rows) => {
      reclamationRow(rows, "reclamation_rollup_complete", "before").reclamation.outcome =
        "read_limit";
    },
    /incomplete reclamation rollup/,
  ],
  [
    "non-EOF read",
    (rows) => {
      reclamationRow(rows, "reclamation_rollup_complete", "after").reclamation.eof = false;
    },
    /incomplete reclamation rollup/,
  ],
  [
    "read-attempt overflow",
    (rows) => {
      reclamationRow(rows, "reclamation_rollup_complete", "before").reclamation.reads = 9;
    },
    /incomplete reclamation rollup/,
  ],
  [
    "oversized input",
    (rows) => {
      reclamationRow(rows, "reclamation_rollup_complete", "after").reclamation.bytes = 16385;
    },
    /incomplete reclamation rollup/,
  ],
  [
    "numeric mismatch",
    (rows) => {
      reclamationRow(rows, "reclamation_rollup_complete", "after").reclamation.rollup.rss_kib++;
    },
    /disagrees with raw/,
  ],
  [
    "duplicate raw offset",
    (rows) => {
      rows.filter(
        (row) => row.phase === "reclamation_rollup_chunk" && row.reclamation.side === "before",
      )[1].reclamation.offset = 0;
    },
    /rollup chunk/,
  ],
  [
    "truncated raw",
    (rows) => {
      reclamationRow(rows, "reclamation_rollup_chunk", "after").reclamation.hex = "00";
    },
    /rollup chunk|missing reclamation rollup/,
  ],
  [
    "bad post-action",
    (rows) => {
      reclamationRow(rows, "reclamation_complete").reclamation.outcome = "malformed";
    },
    /incomplete: malformed/,
  ],
  [
    "elapsed bracket",
    (rows) => {
      const end = reclamationRow(rows, "reclamation_complete");
      end.occurred_us += 2000001;
      end.reclamation.completed_us += 2000001;
      end.reclamation.duration_us += 2000001;
    },
    /bracket order, duration/,
  ],
  [
    "unmatched action start",
    (rows) => {
      rows.splice(
        rows.findIndex((row) => row.phase === "reclamation_action_complete"),
        1,
      );
    },
    /terminal watermark|missing complete/,
  ],
  [
    "record loss",
    (rows) => {
      rows.at(-1).overflow = 1;
    },
    /reported telemetry loss/,
  ],
  [
    "wrong restoration path",
    (_rows, mutations) => {
      mutations.at(-1).paths = ["/foreign.md"];
    },
    /missing its native intake/,
  ],
])("reclamation rejects %s while retaining any performed action", (_name, change, reason) => {
  const report = inspectReclamationStream({ change });
  assert.equal(report.complete, false);
  assert.equal(report.reclamation.targetValid, false);
  assert.match(report.failure.detail, reason);
});

test("legacy post-apply protocol rejects reclamation records", () => {
  const { server, mutations } = reclamationStream();
  const report = inspectRendererLifecycle(server, {
    final: true,
    withAllocator: true,
    withPostApply: true,
    mutations,
  });
  assert.match(report.failure.detail, /startup identity, contract/);
});

test("a gap before the bracket preserves the visible action receipt", () => {
  const { server, mutations } = reclamationStream({
    change(rows) {
      rows.splice(
        rows.findIndex((row) => row.phase === "reclamation_rollup_start"),
        1,
      );
    },
  });
  for (const final of [false, true]) {
    const report = inspectRendererLifecycle(server, {
      final,
      withAllocator: true,
      withPostApply: true,
      reclamationArm: "trim",
      mutations,
      restorationWitness: true,
    });
    assert.equal(report.reclamation.performed, true);
    assert.equal(report.reclamation.actionReceipts.length, 1);
    assert.equal(report.reclamation.targetValid, false);
    if (final) assert.equal(report.complete, false);
  }
});

test("a final performed action survives a lost action-complete record", () => {
  const report = inspectReclamationStream({
    change(rows) {
      rows.splice(
        rows.findIndex((row) => row.phase === "reclamation_action_complete"),
        1,
      );
    },
  });
  assert.equal(report.complete, false);
  assert.equal(report.reclamation.performed, true);
  assert.equal(report.reclamation.actionReceipts.length, 0);
  assert.equal(report.reclamation.terminalReceipts.length, 1);
});

test("an unfinished trim call has an unknown execution outcome", () => {
  const report = inspectReclamationStream({
    change(rows) {
      rows.splice(rows.findIndex((row) => row.phase === "reclamation_action_start") + 1);
    },
  });
  assert.equal(report.complete, false);
  assert.equal(report.reclamation.performed, null);
  assert.equal(report.reclamation.executionUnknown, true);
});

test("the body witness must follow the complete native bracket", () => {
  const report = inspectReclamationStream({
    change(rows, mutations) {
      const action = reclamationRow(rows, "reclamation_complete");
      // Keep the intake's millisecond uncertainty wholly inside the mutation
      // window, while placing its body witness inside the bracket's final rounding.
      action.occurred_us += 5000;
      action.reclamation.completed_us += 5000;
      action.reclamation.duration_us += 5000;
      for (const row of rows) if (row.occurrence > action.occurrence) row.occurred_us += 5000;
      mutations.at(-1).witnessNs = String((100000 + action.reclamation.completed_us) * 1000);
    },
  });
  assert.equal(report.complete, false);
  assert.match(report.failure.detail, /body witness precedes/);
  assert.equal(report.reclamation.performed, true);
});

test("raw rollup parsing rejects truncation, duplicates, overflow and header ambiguity", () => {
  const valid =
    "1000-2000 ---p 00000000 00:00 0 [rollup]\nRss: 5 kB\nAnonymous: 4 kB\nPrivate_Clean: 1 kB\nPrivate_Dirty: 3 kB\nSwap: 0 kB\nAnonHugePages: 0 kB\n";
  assert.equal(parseReclamationRollup(Buffer.from(valid)).values.rss_kib, 5);
  for (const text of [
    valid.slice(0, -1),
    valid + "Rss: 6 kB\n",
    valid.replace("Rss: 5", "Rss: 18446744073709551616"),
    valid.replace("1000-2000", "2000-1000"),
    valid.replace("1000-2000", "1000-10000000000000000"),
    valid.replace("Private_Dirty: 3 kB\n", ""),
    valid.replace("Swap: 0 kB", "Swap: 0 MB"),
    valid + Array.from({ length: 65 }, (_, i) => `Extra${i}: 0 kB\n`).join(""),
  ])
    assert.throws(() => parseReclamationRollup(Buffer.from(text)));
  assert.throws(() => parseReclamationRollup(Buffer.alloc(16385)));
});

test.each([11, 12])(
  "cleanup can reach ordinal thirteen after only %s witnessed body edits",
  (witnessedEdits) => {
    const { server, mutations } = reclamationStream();
    const report = inspectRendererLifecycle(server, {
      final: true,
      withAllocator: true,
      withPostApply: true,
      reclamationArm: "trim",
      mutations: mutations.slice(0, witnessedEdits),
      restorationWitness: false,
      requireCompletePostApply: false,
    });
    assert.equal(report.complete, false);
    assert.equal(report.reclamation.targetValid, false);
    assert.equal(report.reclamation.performed, true);
    assert.equal(report.reclamation.actionReceipts.length, 1);
  },
);

test("losing every bracket row leaves trim execution unknown", () => {
  const report = inspectReclamationStream({
    change(rows) {
      for (let index = rows.length - 1; index >= 0; index--)
        if (
          rows[index].phase?.startsWith("reclamation_") ||
          rows[index].allocator_phase === "reclamation_after"
        )
          rows.splice(index, 1);
    },
  });
  assert.equal(report.complete, false);
  assert.equal(report.reclamation.targetValid, false);
  assert.equal(report.reclamation.performed, null);
  assert.equal(report.reclamation.executionUnknown, true);
  assert.equal(report.reclamation.actionReceipts.length, 0);
});

test("reclamation retains ordinary provenance rows above the lifecycle sink's line bound", () => {
  const report = inspectReclamationStream({
    change(rows) {
      rows.splice(-1, 0, { ev: "observations", observations: "x".repeat(1100) });
    },
  });
  assert.equal(report.failure, null);
  assert.equal(report.reclamation.complete, true);
});

test("reclamation rejects renderer lifecycle records above their native line bound", () => {
  const report = inspectReclamationStream({
    change(rows) {
      reclamationRow(rows, "reclamation_action_complete").extra = "x".repeat(1100);
    },
  });
  assert.match(report.failure.detail, /trace line exceeded 1024 bytes/);
  assert.equal(report.reclamation.performed, true);
  assert.equal(report.reclamation.targetValid, false);
});

test("the lifecycle line bound measures native bytes after any captured log prefix", () => {
  const { server, mutations } = reclamationStream();
  const captured = server.provSince();
  const wrapped = {
    ...server,
    provSince: () => ({
      ...captured,
      lines: captured.lines.map((line) => ({
        ...line,
        text: "external-log-prefix".repeat(60) + line.text,
      })),
    }),
  };
  const report = inspectRendererLifecycle(wrapped, {
    final: true,
    withAllocator: true,
    withPostApply: true,
    reclamationArm: "trim",
    mutations,
    restorationWitness: true,
    requireCompletePostApply: true,
  });
  assert.equal(report.failure, null);
  assert.equal(report.reclamation.complete, true);
});
