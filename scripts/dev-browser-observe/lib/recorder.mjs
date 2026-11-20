import { appendJsonlSync, isoNow } from "./util.mjs";

/** Append-only JSONL recorder: every record is written and fsynced before append() returns. */
export class Recorder {
  constructor(path) {
    this.path = path;
    this.seq = 0;
    this.records = [];
  }

  append(record) {
    const full = { seq: this.seq, at: isoNow(), ...record };
    appendJsonlSync(this.path, full);
    this.seq += 1;
    this.records.push(full);
    return full;
  }
}
