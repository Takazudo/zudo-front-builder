import { isPromise } from "node:util/types";

const MAX_RECORDS = 128;

/**
 * Browser startup's bounded monotonic observations. Parent IDs are explicit
 * so an unrelated controller callback never inherits an outstanding await.
 */
export class BrowserTiming {
  constructor({ clock = process.hrtime.bigint, cap = MAX_RECORDS } = {}) {
    if (!Number.isSafeInteger(cap) || cap < 0 || cap > MAX_RECORDS)
      throw new RangeError("invalid browser timing record cap");
    this.clock = clock;
    this.cap = cap;
    this.nextId = 0;
    this.dropped = 0;
    this.syncSpans = [];
    this.asyncSpans = [];
    this.marks = [];
  }

  record(rows, data) {
    if (this.nextId >= this.cap) {
      this.dropped++;
      return null;
    }
    const row = { id: this.nextId++, ...data };
    rows.push(row);
    return row;
  }

  mark(name) {
    const atNs = String(this.clock());
    this.record(this.marks, { name, atNs });
    return atNs;
  }

  syncSpan(name, parentId, details, operation) {
    const row = this.record(this.syncSpans, {
      ...details,
      name,
      parentId,
      startNs: String(this.clock()),
      endNs: null,
      outcome: "pending",
    });
    if (!row) return operation(null);
    try {
      const result = operation(row);
      row.outcome = "returned";
      return result;
    } catch (error) {
      row.outcome = "threw";
      throw error;
    } finally {
      row.endNs = String(this.clock());
    }
  }

  asyncSpan(name, parentId, operation) {
    const row = this.record(this.asyncSpans, {
      name,
      parentId,
      enterNs: String(this.clock()),
      returnNs: null,
      settledObservedNs: null,
      throwObservedNs: null,
      returnKind: null,
      outcome: "pending",
    });
    if (!row) return operation(null);
    let result;
    try {
      result = operation(row.id);
    } catch (error) {
      row.outcome = "threw";
      row.throwObservedNs = String(this.clock());
      throw error;
    }
    row.returnNs = String(this.clock());
    if (isPromise(result)) {
      row.returnKind = "promise";
      // Observe settlement without awaiting a wrapper or changing the returned
      // Promise. These are callback observations, not exact settlement times.
      try {
        Promise.prototype.then.call(
          result,
          () => {
            row.settledObservedNs = String(this.clock());
            row.outcome = "fulfilled";
          },
          () => {
            row.settledObservedNs = String(this.clock());
            row.outcome = "rejected";
          },
        );
      } catch {
        // A promise subclass can reject attaching an observer via its species.
        row.outcome = "settlement-unobserved";
      }
    } else if (result !== null && (typeof result === "object" || typeof result === "function")) {
      // Reading a possibly user-provided then getter could change the call's
      // behavior. No settlement is observed for a non-native return value.
      row.returnKind = "object-or-function";
      row.outcome = "settlement-unobserved";
    } else {
      row.returnKind = "value";
      row.outcome = "returned";
    }
    return result;
  }

  snapshot() {
    const capturedNs = String(this.clock());
    const copy = (rows) => rows.map((row) => ({ ...row }));
    const pending = [...this.syncSpans, ...this.asyncSpans].filter(
      (row) => row.outcome === "pending",
    ).length;
    const unobserved = this.asyncSpans.filter(
      (row) => row.outcome === "settlement-unobserved",
    ).length;
    const truncated = this.syncSpans.filter(
      (row) => row.pathTruncated || row.resolvedPathTruncated,
    ).length;
    return {
      schema: "dev-browser-observe.browser-timing.v1",
      clock: "node process.hrtime.bigint monotonic ns",
      cap: this.cap,
      capturedNs,
      dropped: this.dropped,
      pending,
      unobserved,
      truncated,
      incomplete: this.dropped + pending + unobserved + truncated > 0,
      syncSpans: copy(this.syncSpans),
      asyncSpans: copy(this.asyncSpans),
      marks: copy(this.marks),
    };
  }
}

export function timeBrowserSync(timing, name, parentId, details, operation) {
  return timing ? timing.syncSpan(name, parentId, details, operation) : operation(null);
}

export function timeBrowserAsync(timing, name, parentId, operation) {
  return timing ? timing.asyncSpan(name, parentId, operation) : operation(null);
}
