export type Phase = "derivation" | "commit" | "effect";

export interface Job {
  readonly id: string;
  readonly phase: Phase;
  readonly active: () => boolean;
  readonly run: () => void;
  readonly report?: (code: string, actual: string) => void;
}

export const MAX_FLUSH_PASSES = 100;
const phases: readonly Phase[] = ["derivation", "commit", "effect"];
let pending = new Set<Job>();
let next = new Set<Job>();
let batchDepth = 0;
let scheduled = false;
let draining = false;
let waiters: Array<{ resolve: () => void; reject: (reason: Error) => void }> = [];

function schedule(): void {
  if (scheduled || draining || batchDepth > 0 || pending.size === 0) return;
  scheduled = true;
  queueMicrotask(() => {
    scheduled = false;
    drain();
  });
}

export function enqueue(job: Job): void {
  if (!job.active()) return;
  (draining ? next : pending).add(job);
  schedule();
}

function drain(): void {
  if (draining || batchDepth > 0) return;
  draining = true;
  let passes = 0;
  try {
    while (pending.size > 0) {
      if (++passes > MAX_FLUSH_PASSES) {
        const jobs = [...pending, ...next].filter((job) => job.active());
        pending.clear();
        next.clear();
        const actual = [...new Set(jobs.map((job) => job.id))].join(", ");
        const affected = new Map<NonNullable<Job["report"]>, Set<string>>();
        for (const job of jobs) {
          if (!job.report) continue;
          const ids = affected.get(job.report) ?? new Set<string>();
          ids.add(job.id);
          affected.set(job.report, ids);
        }
        for (const [report, ids] of affected) {
          try {
            report("ZR_FLUSH_LIMIT", [...ids].join(", "));
          } catch {
            /* A reporter cannot trap the scheduler. */
          }
        }
        const error = new Error(`ZR_FLUSH_LIMIT: ${actual}`);
        if (affected.size === 0) console.error(error);
        for (const waiter of waiters.splice(0)) waiter.reject(error);
        return;
      }
      const pass = pending;
      pending = new Set();
      for (const phase of phases) {
        for (const job of pass) {
          if (job.phase !== phase || !job.active()) continue;
          try {
            job.run();
          } catch (error) {
            if (job.report) {
              try {
                job.report("ZR_SUBSCRIBER_ERROR", String(error));
              } catch {
                /* Keep draining. */
              }
            } else {
              console.error(error);
            }
          }
        }
      }
      pending = next;
      next = new Set();
    }
    for (const waiter of waiters.splice(0)) waiter.resolve();
  } finally {
    draining = false;
    schedule();
  }
}

export function batch<T>(fn: () => T): T {
  batchDepth++;
  try {
    return fn();
  } finally {
    batchDepth--;
    schedule();
  }
}

export function flush(): Promise<void> {
  if (!draining && pending.size === 0) return Promise.resolve();
  const promise = new Promise<void>((resolve, reject) => {
    waiters.push({ resolve, reject });
  });
  schedule();
  return promise;
}
