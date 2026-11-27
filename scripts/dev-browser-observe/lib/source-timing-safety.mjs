import assert from "node:assert/strict";
import { isDeepStrictEqual } from "node:util";
import { sampleRendererMemory } from "./linux-rss-sample.mjs";
import { sameLinuxBirth } from "./linux-birth.mjs";

export const SOURCE_TIMING_PROTOCOL = "linux-source-csec-body.v1";
export const CSEC_EXE_SHA256 = "a7b39da35a93ea4f325e63bbd3d33163d583ac851d1a9c5e9be2ec5608afd9a5";

// Bounded pre-decision alignment (owner decision on #4870/#4878). A decision
// that finds a sample in flight, or a stale sample with the next tick due,
// waits for that ordinary sample to settle and decides on it. It never
// dispatches a sample and never applies a result to an earlier instant.
// coreMs/closureMs are the helper budgets in linux-rss-sample.mjs.
// tickSkewMs is exactly 400 ms by owner decision on #4878. It covers the late
// ticks recorded on WSL: 270.8 ms (aa100 s1-A epoch 6), 231.6 ms (#4862
// rapid-recovery) and 81 ms (#4870 R3). It stays below the sampler's own 1 s
// missed-dispatch limit.
// freshMarginMs: a sample counts as fresh only with this much coverage left, because
// the write site runs synchronous work between the decision and its T0 stamp
// (markAttempt, instability check, stageBudget, performWrite's stat). Measured on
// WSL over 20,000 iterations each: idle p99.9 0.12 ms / max 0.50 ms; with 24 busy
// loops on 12 cores, p99.9 0.19 ms / max 5.5 ms (preemption). 25 ms is about 4.5x
// the loaded maximum. With less left, the decision takes the tick path instead.
export const DECISION_SETTLE = { coreMs: 750, closureMs: 200, tickSkewMs: 400, freshMarginMs: 25 };

export function sourceTimingProfileValid(p) {
  return (
    p.sourceTimingSafety === SOURCE_TIMING_PROTOCOL &&
    p.mode === "source-built" &&
    p.diagnostic === false &&
    p.productSha === "6a4ec653fbb6cb7952b465632d5c58ca18f03e15" &&
    p.productDirtyIdentity == null &&
    !p.rendererMemoryStudy &&
    !p.resourceStudy &&
    !p.tinyAcceptance &&
    !p.startupAcceptance &&
    p.fixture?.kind === "field-notes" &&
    p.fixture.scale === 100 &&
    p.fixture.adapter === "direct-read" &&
    p.fixture.managedCss === true &&
    (p.fixture.placement ?? "external") === "external" &&
    p.fixture.source?.datasetSha256 ===
      "fcaa222e52caaedb411a4136f106d4ddb5eaa7334a3ef03f34ccd282f6bab58a" &&
    isDeepStrictEqual(p.browser, {
      headless: true,
      args: [
        "--disable-background-timer-throttling",
        "--disable-renderer-backgrounding",
        "--disable-backgrounding-occluded-windows",
      ],
      viewport: { width: 1280, height: 800 },
      deviceScaleFactor: 1,
    }) &&
    p.server?.host === "127.0.0.1" &&
    p.server.port?.mode === "ephemeral" &&
    !Object.keys(p.server.env ?? {}).length &&
    !p.server.extraArgs?.length &&
    p.server.readyTimeoutMs === 300000 &&
    p.limits?.rssCeilingKiB === 4194304 &&
    p.limits.wallCeilingMs === 7200000 &&
    p.limits.sampleEveryMs === 1000 &&
    p.timeouts?.attemptMs === 60000 &&
    p.timeouts.supplementalMs === 60000 &&
    p.timeouts.restoreMs === 120000 &&
    p.timeouts.quietMs === 3000 &&
    p.timeouts.stopTermMs === 5000 &&
    p.timeouts.stopKillMs === 3000 &&
    p.restoration?.writeMethod === "in-place" &&
    isDeepStrictEqual(p.scenarios, [
      {
        id: "fn-edit-body",
        set: "workload",
        warmOrder: "none",
        ops: [
          {
            id: "body-05",
            op: "edit-body",
            article: "article-00005",
            writeMethod: "in-place",
            repeat: { warmups: 2, measured: 10 },
          },
        ],
      },
    ])
  );
}

function stamp(v) {
  assert(typeof v === "string" && /^\d+$/.test(v), "invalid monotonic timestamp");
  return BigInt(v);
}

function verifySample(result, handle, image, start) {
  const root = handle.native;
  const live = result.processes?.find((p) => p.pid === root.pid);
  const t = result.timing?.parent;
  const rows = result.processes ?? [];
  assert(
    result.helperResult === "complete" &&
      result.helper?.closed &&
      result.helper.pipesClosed &&
      result.helper.code === 0 &&
      result.helper.signal === null &&
      result.timing.clock.qualified &&
      t.exitNs &&
      t.closeNs &&
      t.stdinCloseNs &&
      t.stdoutCloseNs &&
      t.stderrCloseNs &&
      stamp(t.entryNs) >= start &&
      stamp(t.resultNs) >= stamp(t.entryNs) &&
      stamp(t.resultNs) <= stamp(t.deadlineNs) &&
      stamp(t.deadlineNs) === stamp(t.entryNs) + 750000000n &&
      rows.length === handle.known.size &&
      new Set(rows.map((p) => p.pid)).size === rows.length &&
      rows.every(
        (p) =>
          sameLinuxBirth(p, handle.known.get(p.pid)) &&
          p.ppid === handle.known.get(p.pid).ppid &&
          p.pgid === handle.known.get(p.pid).pgid,
      ) &&
      live?.disposition === "live" &&
      isDeepStrictEqual(live.executable, image) &&
      !result.journalFailures?.length,
    "incomplete source timing RSS ownership/closure",
  );
  const h = result.helper,
    cl = result.timing.clock;
  assert(
    h.birth?.pid === h.pid &&
      h.birth.bootId === root.bootId &&
      h.birth.ppid === root.ppid &&
      h.birth.birthVersion === "linux-proc-v1" &&
      /^\d+$/.test(h.birth.startTicks) &&
      cl.parent?.platform === "linux" &&
      cl.parent.clockId === "CLOCK_MONOTONIC" &&
      cl.helper?.implementation === "clock_gettime(CLOCK_MONOTONIC)" &&
      cl.parent.bootId === root.bootId &&
      cl.helper.bootId === root.bootId &&
      /^time:\[\d+\]$/.test(cl.parent.timeNamespace) &&
      cl.parent.timeNamespace === cl.helper.timeNamespace,
    "unqualified source timing helper birth/clock",
  );
  const entry = stamp(t.entryNs),
    end = stamp(t.resultNs),
    spawn = stamp(h.spawnHrtimeNs);
  assert(
    entry <= spawn &&
      spawn === stamp(t.spawnCallNs) &&
      spawn <= stamp(t.spawnReturnNs) &&
      stamp(t.exitNs) <= stamp(t.closeNs) &&
      t.exitCode === 0 &&
      t.exitSignal === null &&
      ["exitNs", "closeNs", "stdinCloseNs", "stdoutCloseNs", "stderrCloseNs"].every(
        (k) => stamp(t[k]) >= spawn && stamp(t[k]) <= end,
      ),
    "source timing helper exit or stream closure outside accepted interval",
  );
  assert(Object.keys(result.observedRssKiB).every((k) => rows.some((p) => String(p.pid) === k)));
  let sum = 0;
  for (const p of rows) {
    const sources = Object.entries(result.observedRssKiB[p.pid] ?? {});
    assert(
      sources.every(
        ([k, v]) => ["VmRSS", "rollup.Rss"].includes(k) && Number.isSafeInteger(v) && v >= 0,
      ),
    );
    assert.equal(p.rssKiB, sources.length ? Math.max(...sources.map(([, v]) => v)) : null);
    if (p.disposition === "live") {
      const vm = p.statusKiB?.VmRSS,
        rss = p.smapsRollupKiB?.Rss;
      assert(
        Number.isSafeInteger(vm) &&
          vm >= 0 &&
          Number.isSafeInteger(rss) &&
          rss >= 0 &&
          p.rssKiB === Math.max(vm, rss) &&
          result.observedRssKiB?.[p.pid]?.VmRSS === vm &&
          result.observedRssKiB[p.pid]["rollup.Rss"] === rss,
        "missing live RSS",
      );
    } else
      assert(
        ["proved-departure", "retained-terminal-proof"].includes(p.disposition) &&
          sameLinuxBirth(p.terminalProof?.birth, p) &&
          p.terminalProof.birth.ppid === p.ppid &&
          stamp(p.terminalProof.observedNs) <= end &&
          p.terminalProof.pidfdFlags === 0 &&
          [1, 16, 17].includes(p.terminalProof.pollMask),
        "unproved source timing departure",
      );
    if (p.rssKiB !== null) {
      assert(Number.isSafeInteger(p.rssKiB) && p.rssKiB >= 0);
      sum += p.rssKiB;
    }
  }
  assert(
    Number.isSafeInteger(sum) && sum > 0 && sum === result.summedRssKiB,
    "partial source timing RSS",
  );
  return sum;
}

// One regular observer owns startup, the twelve edits and restoration. A
// mutation only consumes a completed sample. It never starts or awaits one.
export function startSourceTimingSafety({
  server,
  executable,
  image,
  limits,
  append,
  abort,
  sample = sampleRendererMemory,
  clock = process.hrtime.bigint,
  workDeadlineNs,
}) {
  const interval = BigInt(limits.sampleEveryMs) * 1000000n;
  const origin = clock();
  let due = origin,
    timer,
    pending,
    stopped = false,
    failure = null,
    secondary = null,
    unresolvedHelper = null,
    pendingHelper = null,
    pendingStartNs = null,
    pendingEpoch = null;
  const dispatchWaiters = new Set();
  const wake = () => {
    for (const w of dispatchWaiters) w();
    dispatchWaiters.clear();
  };
  let dispatched = 0,
    accepted = 0,
    last = null,
    peak = null,
    partialPeak = null;
  function refuse(error) {
    const details = error.details ?? null;
    if (details?.helper && (!details.helper.closed || !details.helper.pipesClosed))
      unresolvedHelper = details.helper;
    if (details?.observedRssKiB) {
      const sources = Object.values(details.observedRssKiB);
      const values = sources
        .map((r) => Object.values(r))
        .filter((r) => r.length && r.every((v) => Number.isSafeInteger(v) && v >= 0));
      if (values.length)
        partialPeak = Math.max(
          partialPeak ?? 0,
          values.reduce((n, v) => n + Math.max(...v), 0),
        );
    }
    const fault = { error: String(error.message ?? error), details, decisionNs: String(clock()) };
    if (failure) {
      secondary ??= fault;
      return;
    }
    failure = fault;
    clearTimeout(timer);
    wake();
    abort(error);
    try {
      append({ type: "source-timing-rss-failure", failure: fault });
    } catch (journalError) {
      fault.journalError = String(journalError);
    }
  }
  function dispatch() {
    if (stopped || failure) return;
    const start = clock(),
      lateness = start - due;
    if (lateness < 0n) {
      timer = setTimeout(dispatch, Math.ceil(Number(-lateness) / 1e6));
      return;
    }
    if (pending || lateness >= interval) {
      refuse(
        Error(
          pending ? "source timing sampler overlap" : "source timing sampler missed a dispatch",
        ),
      );
      return;
    }
    const epoch = ++dispatched,
      scheduled = due;
    due += interval;
    // Publish the outstanding operation before the backend or evidence sink
    // can fail. Stop always observes settlement of this same chain.
    const owned = Promise.withResolvers();
    pending = owned.promise
      .then((result) => {
        try {
          const sum = verifySample(result, server.handle, image, start);
          const t = result.timing.parent;
          peak = Math.max(peak ?? 0, sum);
          if (sum > limits.rssCeilingKiB)
            throw Object.assign(Error("source timing RSS ceiling exceeded"), { details: result });
          append({
            type: "source-timing-rss-sample",
            epoch,
            scheduledNs: String(scheduled),
            startNs: String(start),
            latenessNs: String(lateness),
            result,
            ...(failure && { afterFailure: true }),
          });
          const acceptedNs = clock();
          assert(
            acceptedNs <= stamp(t.deadlineNs),
            "source timing acceptance exceeded core deadline",
          );
          if (!failure) {
            accepted++;
            last = {
              epoch,
              startNs: String(start),
              deadlineNs: String(scheduled + interval),
              acceptedNs: String(acceptedNs),
              summedRssKiB: sum,
            };
          }
        } catch (error) {
          error.details ??= result;
          throw error;
        }
      })
      .catch(refuse)
      .finally(() => {
        pendingHelper = null;
        pending = null;
        pendingStartNs = null;
        pendingEpoch = null;
      });
    pendingStartNs = start;
    pendingEpoch = epoch;
    try {
      owned.resolve(
        sample(server, executable, {
          onHelperSpawn: (h) => {
            pendingHelper = h;
          },
        }),
      );
    } catch (error) {
      owned.reject(error);
    }
    timer = setTimeout(dispatch, Math.max(1, Math.ceil(Number(due - clock()) / 1e6)));
    wake();
    return pending;
  }
  // Resolve at untilNs (injected clock), or earlier on wake or abort.
  const delay = (untilNs, register, signal) =>
    new Promise((resolve) => {
      const left = untilNs - clock();
      const t = setTimeout(done, Math.max(0, Math.ceil(Number(left) / 1e6)));
      function done() {
        clearTimeout(t);
        if (register) dispatchWaiters.delete(done);
        signal?.removeEventListener("abort", done);
        resolve();
      }
      if (register) dispatchWaiters.add(done);
      signal?.addEventListener("abort", done, { once: true });
    });
  async function awaitDecision({ site = null, capNs = null, signal = null } = {}) {
    const enteredNs = clock();
    const meta = {
      site,
      enteredNs: String(enteredNs),
      pendingAtDecision: Boolean(pending),
      // A pending sample almost always means the previous one is already stale:
      // its deadline is the next grid slot.
      staleAtDecision: !last || enteredNs >= BigInt(last.deadlineNs),
      fromEpoch: last?.epoch ?? null,
      settledEpoch: last?.epoch ?? null,
      waitedNs: "0",
      outcome: null,
      boundNs: null,
    };
    const caps = [capNs, workDeadlineNs].filter((x) => x !== null && x !== undefined);
    const cap = caps.length ? caps.reduce((a, b) => (a < b ? a : b)) : null;
    const limit = (ns) => (cap !== null && cap < ns ? cap : ns);
    const deny = (reason) => {
      meta.outcome = reason;
      meta.waitedNs = String(clock() - enteredNs);
      refuse(Object.assign(Error("source timing decision refused: " + reason), { decision: meta }));
      throw Object.assign(Error(failure.error), { decision: meta });
    };
    if (failure) throw Object.assign(Error(failure.error), { decision: meta });
    if (stopped) deny("stopped");
    const marginNs = BigInt(DECISION_SETTLE.freshMarginMs) * 1000000n;
    if (!pending && last && enteredNs + marginNs < BigInt(last.deadlineNs)) {
      meta.outcome = "fresh";
      return meta;
    }
    meta.withinFreshMargin = !pending && Boolean(last) && enteredNs < BigInt(last.deadlineNs);
    if (!pending) {
      // Stale with nothing in flight: only the already scheduled tick may serve.
      const tickBy = limit(due + BigInt(DECISION_SETTLE.tickSkewMs) * 1000000n);
      meta.boundNs = String(tickBy);
      while (!pending && !failure && !stopped && clock() < tickBy) {
        signal?.throwIfAborted();
        await delay(tickBy, true, signal);
      }
      if (failure) throw Object.assign(Error(failure.error), { decision: meta });
      if (stopped) deny("stopped");
      if (!pending) deny(cap !== null && tickBy === cap ? "deadline-before-tick" : "tick-late");
      // A blocked loop runs the late dispatch first; its own start decides.
      if (pendingStartNs > tickBy)
        deny(cap !== null && tickBy === cap ? "deadline-before-tick" : "tick-late");
      meta.tickStartNs = String(pendingStartNs);
    }
    const expected = pendingEpoch;
    const settleBy = limit(
      pendingStartNs + BigInt(DECISION_SETTLE.coreMs + DECISION_SETTLE.closureMs) * 1000000n,
    );
    meta.boundNs = String(settleBy);
    const inFlight = pending;
    let settled = false;
    inFlight.then(() => {
      settled = true;
    });
    while (!settled && !failure && clock() < settleBy) {
      signal?.throwIfAborted();
      await Promise.race([inFlight, delay(settleBy, false, signal)]);
    }
    if (failure) throw Object.assign(Error(failure.error), { decision: meta });
    if (!settled)
      deny(cap !== null && settleBy === cap ? "deadline-before-settle" : "never-settles");
    if (stopped) deny("stopped");
    const now = clock();
    signal?.throwIfAborted();
    if (!last || last.epoch !== expected || now + marginNs >= BigInt(last.deadlineNs) || pending)
      deny("stale-after-settle");
    if (!server.isAlive().alive) deny("server-not-alive");
    meta.outcome = "settled";
    meta.settledEpoch = last.epoch;
    meta.waitedNs = String(now - enteredNs);
    return meta;
  }
  // Wait if needed, then take the synchronous decision on the settled sample.
  async function decide(options = {}) {
    const meta = await awaitDecision(options);
    const accepted = current();
    return { ...accepted, decision: { ...meta, decisionNs: accepted.decisionNs } };
  }
  const first = dispatch();
  function current() {
    const alive = !failure && !stopped && !pending && server.isAlive().alive;
    const stamp = clock();
    if (
      !failure &&
      (!alive ||
        !last ||
        stamp >= BigInt(last.deadlineNs) ||
        (workDeadlineNs !== undefined && stamp >= workDeadlineNs))
    ) {
      refuse(Error("source timing observation pending, stale, missing or stopped"));
    }
    if (failure) throw Error(failure.error);
    return { ...last, decisionNs: String(stamp) };
  }
  return {
    async ready() {
      await first;
      if (failure) throw Error(failure.error);
    },
    current,
    awaitDecision,
    decide,
    latest() {
      if (failure) throw Error(failure.error);
      return last;
    },
    get unresolvedHelper() {
      return unresolvedHelper ?? pendingHelper;
    },
    async stop() {
      if (!stopped && !failure && clock() >= due + interval)
        refuse(Error("source timing sampler missed a dispatch before stop"));
      stopped = true;
      clearTimeout(timer);
      wake();
      await pending;
      return failure;
    },
    report() {
      return {
        protocol: SOURCE_TIMING_PROTOCOL,
        dispatched,
        accepted,
        peakSummedOwnedRssKiB: peak,
        peakFailedObservedSubtotalKiB: partialPeak,
        failure,
        secondary,
      };
    },
  };
}

// Recompute the evidence required by the formal reader from the original
// samples and mutation receipts. A malformed or incomplete timestamp fails
// admission instead of escaping from aggregation.
export function sourceTimingEvidenceValid({ records, receipt, summary, completion }) {
  try {
    const start = records.find((r) => r.type === "run-start");
    const protocol = SOURCE_TIMING_PROTOCOL;
    assert(
      start?.sourceTimingSafety === protocol &&
        sourceTimingProfileValid(receipt.profile) &&
        receipt.sourceTimingSafety === protocol &&
        completion.sourceTimingSafety === protocol &&
        completion.diagnostic === false,
    );
    const report = summary.sourceTimingSafety;
    assert(
      report?.protocol === protocol &&
        report.failure === null &&
        report.secondary === null &&
        !records.some((r) => r.type === "source-timing-rss-failure"),
    );
    assert(
      Number.isSafeInteger(completion.controllerExitMonotonicMs) &&
        Number.isSafeInteger(completion.startedMonotonicMs) &&
        Number.isSafeInteger(completion.deadlineMonotonicMs) &&
        completion.startedMonotonicMs <= completion.controllerExitMonotonicMs &&
        completion.controllerExitMonotonicMs <= completion.deadlineMonotonicMs &&
        completion.deadlineMonotonicMs - completion.startedMonotonicMs ===
          receipt.profile.limits.wallCeilingMs,
    );
    const native = receipt.server.native,
      image = receipt.sourceTimingImage;
    assert(
      native.ppid === completion.controllerPid &&
        image.path === receipt.buildReceipt.exe.path &&
        receipt.sourceTimingImageSha256 === CSEC_EXE_SHA256 &&
        receipt.buildReceipt.exe.sha256 === CSEC_EXE_SHA256 &&
        receipt.buildReceipt.source.sha === receipt.profile.productSha,
    );
    const known = new Map([[native.pid, native]]),
      seen = new Set();
    const samples = records.filter((r) => r.type === "source-timing-rss-sample");
    assert(
      samples.length > 0 &&
        samples.length === report.accepted &&
        samples.length === report.dispatched,
    );
    const origin = stamp(samples[0].scheduledNs),
      interval = 1000000000n;
    let peak = 0,
      previousEnd = 0n;
    for (const [i, r] of samples.entries()) {
      const raw = r.result,
        scheduled = stamp(r.scheduledNs),
        began = stamp(r.startNs);
      assert(
        r.epoch === i + 1 &&
          !r.afterFailure &&
          scheduled === origin + BigInt(i) * interval &&
          began >= scheduled &&
          began - scheduled === stamp(r.latenessNs) &&
          began - scheduled < interval &&
          began >= previousEnd,
      );
      for (const b of raw.verifiedDiscoveries) {
        assert(!known.has(b.pid) && b.binding?.pidfdFlags === 0 && b.binding.fdinfoPid === b.pid);
        known.set(b.pid, b);
      }
      const sum = verifySample(raw, { native, known }, image, began);
      assert(sum <= receipt.profile.limits.rssCeilingKiB);
      previousEnd = stamp(raw.timing.parent.resultNs);
      const birth = raw.helper.birth,
        key = birth.bootId + "/" + birth.pid + "/" + birth.startTicks;
      assert(!seen.has(key));
      seen.add(key);
      peak = Math.max(peak, sum);
    }
    assert(peak === report.peakSummedOwnedRssKiB);
    const attempts = records.filter((r) => r.type === "attempt");
    assert(attempts.length === 12);
    for (const [i, a] of attempts.entries()) {
      assert(
        a.outcome === "success" &&
          a.label === (i < 2 ? "warmup" : "measured") &&
          Number.isSafeInteger(a.sourceTimingWriteEpoch) &&
          /^\d+$/.test(a.t0Ns) &&
          /^\d+$/.test(a.sourceTimingAcceptedNs),
      );
      const epoch = a.sourceTimingWriteEpoch,
        r = samples[epoch - 1],
        t = stamp(a.t0Ns),
        ack = stamp(a.sourceTimingAcceptedNs);
      assert(
        r &&
          r.seq < a.seq &&
          stamp(r.result.timing.parent.resultNs) <= ack &&
          ack <= stamp(r.result.timing.parent.deadlineNs) &&
          ack <= t &&
          t < stamp(r.scheduledNs) + interval &&
          (!samples[epoch] || t < stamp(samples[epoch].startNs)),
      );
    }
    return true;
  } catch {
    return false;
  }
}
