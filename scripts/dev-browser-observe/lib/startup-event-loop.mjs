import { monitorEventLoopDelay, performance } from "node:perf_hooks";

const resolutionMs = 20;

// A single startup histogram and ELU window. The next already scheduled
// before-edits await gives the monitor a chance to observe the last startup
// turn. If no reading arrived on that turn or later, say so explicitly.
export class StartupEventLoop {
  constructor(clock = process.hrtime.bigint) {
    this.clock = clock;
    this.report = {
      resolutionMs,
      histogramScope: "cumulative since enable",
      enableCallNs: null,
      enableReturnNs: null,
      enabled: null,
      browserReadyCountObservedNs: null,
      countAtBrowserReady: null,
      exceedsAtBrowserReady: null,
      disableCallNs: null,
      disableReturnNs: null,
      disabled: null,
      stopReason: "not-started",
      count: null,
      exceeds: null,
      maxNs: null,
      histogramStatus: "unavailable",
      histogramUnavailableReason: "not-started",
      trailingObservation: "unavailable",
      utilizationStartObservedNs: null,
      utilizationEndObservedNs: null,
      utilizationStart: null,
      utilizationEnd: null,
      utilizationDelta: null,
      utilizationStatus: "unavailable",
    };
  }

  start(signal) {
    const r = this.report;
    this.histogram = monitorEventLoopDelay({ resolution: resolutionMs });
    r.enableCallNs = String(this.clock());
    r.enabled = this.histogram.enable();
    r.enableReturnNs = String(this.clock());
    r.stopReason = "running";
    r.utilizationStart = performance.eventLoopUtilization();
    r.utilizationStartObservedNs = String(this.clock());
    this.signal = signal;
    this.onAbort = () => this.stop("cancelled");
    if (signal?.aborted) this.onAbort();
    else signal?.addEventListener("abort", this.onAbort, { once: true });
  }

  browserReady() {
    if (!this.histogram) return;
    const r = this.report;
    r.countAtBrowserReady = this.histogram.count;
    r.exceedsAtBrowserReady = this.histogram.exceeds;
    r.browserReadyCountObservedNs = String(this.clock());
  }

  stop(reason) {
    if (!this.histogram) return;
    this.signal?.removeEventListener("abort", this.onAbort);
    const r = this.report;
    const histogram = this.histogram;
    this.histogram = null;
    r.stopReason = reason;
    r.disableCallNs = String(this.clock());
    r.disabled = histogram.disable();
    r.disableReturnNs = String(this.clock());
    r.utilizationEnd = performance.eventLoopUtilization();
    r.utilizationEndObservedNs = String(this.clock());
    const delta = performance.eventLoopUtilization(r.utilizationEnd, r.utilizationStart);
    if (Number.isFinite(delta.utilization) && delta.active + delta.idle > 0) {
      r.utilizationDelta = delta;
      r.utilizationStatus = "observed";
    }
    r.count = histogram.count;
    r.exceeds = histogram.exceeds;
    if (r.count > 0) r.maxNs = histogram.max;
    r.histogramStatus = r.exceeds > 0 ? "overflow" : r.count > 0 ? "observed" : "unavailable";
    r.histogramUnavailableReason = r.count > 0 ? null : "no-in-range-samples";
    if (r.countAtBrowserReady !== null)
      r.trailingObservation =
        r.count > r.countAtBrowserReady || r.exceeds > r.exceedsAtBrowserReady
          ? "post-browser-ready-sample-observed"
          : "no-post-browser-ready-sample";
  }
}
