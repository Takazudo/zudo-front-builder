import { timed } from "./controller-timing.mjs";
import { appendJsonlSync, isoNow } from "./util.mjs";

/**
 * Append-only JSONL recorder. Without a sink every record is written and fsynced
 * before append() returns. With a DurableSink (the opt-in durable mode, O1′/O6)
 * the record is written in order before append() returns and becomes durable at
 * the next barrier.
 */
export class Recorder {
  constructor(path, timing, sink = null) {
    this.path = path;
    this.timing = timing;
    this.sink = sink;
    this.seq = 0;
    this.records = [];
  }

  append(record) {
    const full = { seq: this.seq, at: isoNow(), ...record };
    if (this.sink) {
      const line = JSON.stringify(full) + "\n";
      timed(this.timing, "append", { seq: this.seq, recordType: record.type }, () =>
        timed(this.timing, "write", {}, () => this.sink.append(this.path, line)),
      );
      this.sink.lastSeq = full.seq;
    } else
      timed(this.timing, "append", { seq: this.seq, recordType: record.type }, () =>
        appendJsonlSync(this.path, full, this.timing),
      );
    this.seq += 1;
    this.records.push(full);
    return full;
  }
}
