export function fakeRecorder() {
  const records = [];
  return {
    records,
    append(rec) {
      const full = { seq: records.length, ...rec };
      records.push(full);
      return full;
    },
  };
}

export function fakeClock() {
  let ns = 1_000_000_000n;
  return {
    now() {
      ns += 1_000_000n;
      return ns;
    },
  };
}

export function fakeServer({
  exitAfterWrites = null,
  exit = { exitCode: 1, signalCode: null },
} = {}) {
  let resolveExit;
  const exited = new Promise((r) => {
    resolveExit = r;
  });
  const s = {
    exit: null,
    rss: 1000,
    isAlive: () => ({ alive: s.exit === null, exit: s.exit }),
    pid: () => 4242,
    waitExit: () => exited,
    sampleResources: () => ({ summedRssKiB: s.rss }),
    die(e = exit) {
      s.exit = e;
      resolveExit(e);
    },
    exitAfterWrites,
  };
  return s;
}

/** behaviors: one entry per measured wait, consumed in order. */
export function fakePage({
  behaviors = [],
  preconditionTrue = () => false,
  supplemental = () => ({ ok: true }),
  liveCheck = () => ({ ok: true }),
  onWait,
} = {}) {
  const p = {
    docUUID: "doc-1",
    prepared: [],
    armed: [],
    disarms: 0,
    evidence: { mainFrameNavigations: 0, newDocuments: 0 },
    async prepare(path) {
      p.prepared.push(path);
      return { docUUID: p.docUUID };
    },
    async arm(token, predicate) {
      if (preconditionTrue(predicate))
        return { armed: false, precondition: "true-before-write", docUUID: p.docUUID };
      p.armed.push(token);
      return { armed: true, docUUID: p.docUUID };
    },
    markAttempt() {
      p.evidence = { mainFrameNavigations: 0, newDocuments: 0 };
    },
    collectAttempt() {
      return p.evidence;
    },
    async disarm() {
      p.disarms += 1;
    },
    waitNotify(token, _timeoutMs, onLate) {
      if (onWait) {
        const r = onWait(token, onLate, p);
        if (r) return r;
      }
      const b = behaviors.shift() ?? { kind: "timeout" };
      for (const late of b.late ?? []) onLate(late);
      if (b.evidence) p.evidence = b.evidence;
      if (b.kind === "notify")
        return Promise.resolve({
          kind: "notify",
          payload: { token, docUUID: b.docUUID ?? p.docUUID },
          t1: b.t1,
        });
      if (b.kind === "never") return new Promise(() => {});
      return Promise.resolve({ kind: "timeout" });
    },
    async supplemental(check) {
      return supplemental(check);
    },
    async liveCheck(check) {
      return liveCheck(check);
    },
  };
  return p;
}

export function fakeFs({ failWrite = false, failRestore = false, clockForT1 } = {}) {
  const files = { "a.md": "v0", "b.md": "v0" };
  const f = {
    files,
    writes: [],
    restores: 0,
    lastT0: null,
    prepareAttempt(op) {
      const token = `tok-${op.id ?? op.op}`;
      return {
        token,
        targetPath: `/articles/${op.article ?? "a"}`,
        predicate: { kind: "text", selector: "[x]", includes: token },
        supplemental: op.supplementalChecks ?? [],
        write: { kind: "writeFile", path: `${op.article ?? "a"}.md`, bytes: token },
      };
    },
    performWrite(w, onT0) {
      if (failWrite) throw new Error("EACCES: simulated write failure");
      const t0 = onT0();
      f.lastT0 = t0;
      files[w.path] = w.bytes;
      f.writes.push(w.path);
      if (clockForT1) clockForT1(t0);
      return t0;
    },
    snapshot() {
      return { ...files };
    },
    restore(snap) {
      f.restores += 1;
      if (failRestore) return { restored: [], mismatches: [{ path: "a.md", want: "x", got: "y" }] };
      for (const k of Object.keys(files)) delete files[k];
      Object.assign(files, snap);
      return { restored: Object.keys(snap), mismatches: [] };
    },
    liveChecks() {
      return [{ path: "/articles/a", predicate: { kind: "text", selector: "[x]", equals: "v0" } }];
    },
  };
  return f;
}

export function deps({
  page,
  server = fakeServer(),
  fs = fakeFs(),
  limits = {},
  recorder = fakeRecorder(),
  clock = fakeClock(),
}) {
  return {
    recorder,
    clock,
    server,
    page,
    fs,
    limits,
    timeouts: { attemptMs: 50, supplementalMs: 50, restoreMs: 50 },
  };
}
