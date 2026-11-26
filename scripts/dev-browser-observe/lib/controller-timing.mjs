// In-memory observations avoid writing to the same synchronous sink being measured.
// Persist once at shutdown. Stage ids preserve nesting without summing inclusive spans.
export class ControllerTiming {
  constructor(clock = process.hrtime.bigint) {
    this.clock = clock;
    this.phase = "startup";
    this.activeEdit = null;
    this.parentId = null;
    this.report = {
      schema: "dev-browser-observe.controller-timing.v1",
      clock: "node process.hrtime.bigint monotonic ns",
      intervalMs: 1000,
      dispatches: [],
      stages: [],
      dropped: 0,
    };
  }

  stage(name, details, operation) {
    const stages = this.report.stages;
    if (stages.length >= 32_768) {
      this.report.dropped++;
      return operation();
    }
    const row = {
      id: stages.length,
      parentId: this.parentId,
      name,
      phase: this.phase,
      activeEdit: this.activeEdit,
      ...details,
      startNs: String(this.clock()),
    };
    stages.push(row);
    this.parentId = row.id;
    try {
      return operation(row);
    } finally {
      row.endNs = String(this.clock());
      this.parentId = row.parentId;
    }
  }

  dispatch(data) {
    if (this.report.dispatches.length >= 512) {
      this.report.dropped++;
      return null;
    }
    const row = { phase: this.phase, activeEdit: this.activeEdit, ...data };
    this.report.dispatches.push(row);
    return row;
  }
}

export function timed(timing, name, details, operation) {
  return timing ? timing.stage(name, details, operation) : operation();
}
