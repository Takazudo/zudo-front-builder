import { closeSync, fsync, fsyncSync, openSync, renameSync, unlinkSync, writeSync } from "node:fs";
import { dirname } from "node:path";
import { timed } from "./controller-timing.mjs";
import { appendJsonlSync, writeJsonSync } from "./util.mjs";

// Durable recorder mode (#4862 r6; durability-design r2, owner decisions O1′,
// O2′, O5, O6 of 2026-11-29). Rows are written synchronously and in order, so a
// controller crash loses nothing; their fsync runs on the threadpool and is
// awaited only at the barriers (B-pre, B-clean, B-exit), which lie outside every
// cadence or sampler deadline window. Only the original-family profiles opt in;
// with no sink every writer below keeps today's synchronous fsync path.

const fsyncAsync = (fd) =>
  new Promise((resolve, reject) => fsync(fd, (error) => (error ? reject(error) : resolve())));

export class DurableSink {
  constructor({ clock = () => process.hrtime.bigint() } = {}) {
    this.clock = clock;
    this.files = new Map();
    this.renamed = new Set();
    this.pending = new Set();
    this.written = 0;
    this.durable = 0;
    this.lastSeq = null;
    this.inflight = null;
    this.queued = null;
    this.closed = false;
    this.failure = null;
  }

  // Ordered write without fsync; the next barrier makes it durable.
  append(path, line) {
    let fd = this.files.get(path);
    if (fd === undefined) {
      fd = openSync(path, "a");
      this.files.set(path, fd);
    }
    writeSync(fd, line);
    this.written++;
  }

  // A file replaced by rename (the admission intent): the barrier fsyncs the
  // new inode and its directory.
  noteRenamed(path) {
    this.renamed.add(path);
    this.written++;
  }

  // Another durability operation (the receipt checkpoint) that a barrier awaits;
  // its failure fails the round like a fsync error (review-2 item 9).
  track(promise) {
    this.pending.add(promise);
    this.written++;
    promise.then(
      () => this.pending.delete(promise),
      (error) => {
        this.pending.delete(promise);
        this.failure ??= Object.assign(new Error("durable sink failed: " + error.message), {
          cause: error,
        });
      },
    );
    return promise;
  }

  // Resolves once a fsync round that started after the latest write completed.
  // One round is in flight at a time; a write made during it is covered by the
  // next round, which every later caller shares.
  // Sticky failure (review-2 item 1): Linux reports a writeback error once per
  // open file description, so after one failed round a later fsync on the same fd
  // can return 0 although the failed pages are gone. Every later barrier fails.
  barrier() {
    if (this.failure) return Promise.reject(this.failure);
    if (this.inflight && this.inflight.gen === this.written) return this.inflight.promise;
    if (this.inflight) {
      this.queued ??= this.inflight.promise
        .catch(() => {})
        .then(() => {
          this.queued = null;
          if (this.failure) throw this.failure;
          return this.round();
        });
      return this.queued;
    }
    return this.round();
  }

  round() {
    const gen = this.written;
    const throughSeq = this.lastSeq;
    const startNs = this.clock();
    const renamed = [...this.renamed];
    this.renamed.clear();
    const syncPath = async (path) => {
      let fd;
      try {
        fd = openSync(path, "r");
      } catch (error) {
        if (error.code === "ENOENT") return;
        throw error;
      }
      try {
        await fsyncAsync(fd);
      } finally {
        closeSync(fd);
      }
    };
    const work = [
      ...[...this.files.values()].map(fsyncAsync),
      ...renamed.flatMap((path) => [syncPath(path), syncPath(dirname(path))]),
      ...this.pending,
    ];
    const round = { gen };
    round.promise = Promise.all(work)
      .then(() => {
        if (gen > this.durable) this.durable = gen;
        return { startNs: String(startNs), endNs: String(this.clock()), throughSeq };
      })
      .catch((error) => {
        for (const path of renamed) this.renamed.add(path);
        this.failure ??= Object.assign(new Error("durable sink failed: " + error.message), {
          cause: error,
        });
        throw this.failure;
      })
      .finally(() => {
        if (this.inflight === round) this.inflight = null;
      });
    this.inflight = round;
    return round.promise;
  }

  // Run end, after the sampler and server have stopped: no window is open, so
  // the remaining tail is made durable synchronously and the fds are closed.
  close() {
    if (this.closed) return;
    this.closed = true;
    try {
      for (const fd of this.files.values()) fsyncSync(fd);
    } finally {
      for (const fd of this.files.values()) closeSync(fd);
      this.files.clear();
    }
    if (this.failure) throw this.failure;
  }
}

/**
 * One barrier point. It expires on the run's existing work deadline (O2′; review
 * re-check R2-1), or earlier when the caller passes its own stage bound: B-clean
 * uses min(cleanup entry + restoreMs, work deadline) (review-2 item 2). It always
 * appends a `durability` row: on success `throughSeq` is the last row covered.
 */
export function createBarrierPoint({
  sink,
  recorder,
  deadlineNs: workDeadlineNs = null,
  timing = null,
}) {
  return async (point, { boundMs = null } = {}) => {
    const startNs = sink.clock();
    const stageNs = boundMs === null ? null : startNs + BigInt(boundMs) * 1_000_000n;
    const deadlineNs =
      stageNs === null
        ? workDeadlineNs
        : workDeadlineNs === null || stageNs < workDeadlineNs
          ? stageNs
          : workDeadlineNs;
    let timer;
    let outcome;
    try {
      const expiry =
        deadlineNs === null
          ? null
          : new Promise((resolve) => {
              const ms = Math.max(0, Math.ceil(Number(deadlineNs - startNs) / 1e6));
              timer = setTimeout(() => resolve({ expired: true }), ms);
            });
      outcome = await timed(timing, "durability.barrier", { barrier: point }, () =>
        Promise.race([sink.barrier().then((done) => ({ done })), ...(expiry ? [expiry] : [])]),
      );
    } catch (error) {
      outcome = { error: String(error?.message ?? error) };
    } finally {
      clearTimeout(timer);
    }
    const row = {
      type: "durability",
      barrier: point,
      ...(deadlineNs !== null && { deadlineNs: String(deadlineNs) }),
      startNs: String(startNs),
      endNs: String(sink.clock()),
      throughSeq: outcome.done ? outcome.done.throughSeq : null,
      ...(outcome.done && { fsyncStartNs: outcome.done.startNs, fsyncEndNs: outcome.done.endNs }),
      ...(outcome.expired && { expired: true, reason: "durability barrier expired" }),
      ...(outcome.error && { error: outcome.error, reason: "durability barrier failed" }),
    };
    recorder.append(row);
    return { ok: Boolean(outcome.done), ...row };
  };
}

/** The ownership registry: synchronous fsync per row, or write plus barrier (O5). */
export function createOwnershipJournal(path, { sink = null, timing = null } = {}) {
  return {
    append(row) {
      if (!sink) return appendJsonlSync(path, row);
      timed(timing, "registry.append", { rowType: row.type }, () =>
        sink.append(path, JSON.stringify(row) + "\n"),
      );
    },
  };
}

/**
 * The receipt writer (design §2d, review D7). Without a sink every write is
 * today's `writeJsonSync`. With one, a checkpoint fsyncs on the threadpool and
 * renames only after the previous write landed; a final write first awaits every
 * checkpoint still in flight. Each write uses its own temp name, so a late
 * checkpoint can neither truncate a newer temp nor rename over the newer receipt.
 */
export function createReceiptWriter(path, { sink = null, timing = null } = {}) {
  let n = 0;
  let inflight = Promise.resolve();
  let checkpointFailure = null;
  const temp = () => `${path}.tmp-${process.pid}-${++n}`;
  return {
    checkpoint(value) {
      if (!sink) return writeJsonSync(path, value);
      const text = `${JSON.stringify(value, null, 2)}\n`;
      const tmp = temp();
      const fd = timed(timing, "receipt.open", {}, () => openSync(tmp, "w"));
      timed(timing, "receipt.write", {}, () => writeSync(fd, text));
      const previous = inflight;
      const landed = (async () => {
        try {
          try {
            await fsyncAsync(fd);
          } finally {
            closeSync(fd);
          }
          await previous;
          renameSync(tmp, path);
        } catch (error) {
          // A failed checkpoint leaves no temp behind and fails the evidence
          // (review-2 item 9); the final write still lands.
          checkpointFailure ??= error;
          try {
            unlinkSync(tmp);
          } catch {
            // already renamed or never created
          }
          throw error;
        }
      })();
      inflight = landed.catch(() => {});
      return sink.track(landed);
    },
    async write(value) {
      if (!sink) return writeJsonSync(path, value);
      await inflight;
      const tmp = temp();
      timed(timing, "receipt.final", {}, () => {
        const fd = openSync(tmp, "w");
        try {
          writeSync(fd, `${JSON.stringify(value, null, 2)}\n`);
          fsyncSync(fd);
        } finally {
          closeSync(fd);
        }
        renameSync(tmp, path);
      });
      if (checkpointFailure)
        throw new Error(
          "receipt checkpoint failed: " + (checkpointFailure.message ?? checkpointFailure),
        );
    },
  };
}
