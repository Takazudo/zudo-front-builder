import { timed } from "./controller-timing.mjs";
import { appendJsonlSync, isoNow } from "./util.mjs";

/** Append-only JSONL recorder: every record is written and fsynced before append() returns. */
export class Recorder {
  constructor(path, timing) {
    this.path = path;
    this.timing = timing;
    this.seq = 0;
    this.records = [];
  }

  append(record) {
    const full = { seq: this.seq, at: isoNow(), ...record };
    timed(this.timing, "append", { seq: this.seq, recordType: record.type }, () =>
      appendJsonlSync(this.path, full, this.timing),
    );
    this.seq += 1;
    this.records.push(full);
    return full;
  }
}
