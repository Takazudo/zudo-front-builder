import assert from "node:assert/strict";
import { EventEmitter } from "node:events";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test, vi } from "vitest";
import { blankBrowserAudit, runOriginalFamilyBatch } from "../lib/fixed-family-admission.mjs";
import { fieldNotesOps, materializeFieldNotes } from "../lib/fieldnotes.mjs";
import { HARNESS_DIR } from "../lib/util.mjs";
import { profileSchemaFor, semanticErrors } from "../lib/profile.mjs";
import { validateAs } from "../lib/schema.mjs";
import { deps, fakePage } from "./fakes.mjs";
import { makeFakeSource } from "./fieldnotes-fake.mjs";

const op = {
  id: "rapid-add-delete",
  op: "rapid-sequence",
  variant: "add-then-delete",
  writeMethod: "create",
  spacingMs: 100,
};
const scenario = { id: "fn-rapid-add-delete", set: "workload", warmOrder: "none", ops: [op] };
test("the normal profile validator admits only exact original family cells", () => {
  const p = JSON.parse(
    readFileSync(
      join(HARNESS_DIR, "profiles/field-notes-fixed-zfb-original-memoized-100.example.json"),
    ),
  );
  p.fixedFamilyAdmission = "blank-before-custody.v1";
  p.fixedRssSafety = "linux-fixed-zfb.v4";
  p.scenarios = [scenario];
  p.server.readyTimeoutMs = 300000;
  p.limits.wallCeilingMs = 7200000;
  Object.assign(p.timeouts, { attemptMs: 60000, supplementalMs: 60000, restoreMs: 120000 });
  const check = (q) => [...validateAs(profileSchemaFor(q), q), ...semanticErrors(q)];
  assert.deepEqual(check(p), []);
  for (const mutate of [
    (q) => (q.fixedRssSafety = "linux-fixed-zfb.v3"),
    (q) => (q.framework = "next"),
    (q) => (q.fixture.adapter = "recovered-snapshot"),
    (q) => (q.scenarios[0].ops[0].spacingMs = 51),
    (q) => (q.scenarios[0].warmOrder = "index-first"),
  ]) {
    const q = structuredClone(p);
    mutate(q);
    assert(check(q).length);
  }
});
test.each([
  "released",
  "final-cadence-crossing",
  "idle-transient-in-journal",
  "snapshot-during-settlement",
  "write-during-settlement",
  "no-release",
  "abort",
  "request",
  "wrong-population",
  "navigated-during-journal",
  "request-during-journal",
  "topology-after-journal",
  "image-after-journal",
  "fails-after-journal",
])("normal family batch at blank boundary: %s", async (mode) => {
  const temp = mkdtempSync(join(tmpdir(), "family-admission-"));
  try {
    const src = join(temp, "src"),
      source = makeFakeSource(src, 20);
    const m = materializeFieldNotes({
      fixtureDir: join(HARNESS_DIR, "fixtures/field-notes"),
      sourceDir: src,
      workDir: join(temp, "work"),
      adapter: "original-memoized",
      expectedDatasetSha256: source.datasetSha256,
    });
    const audit = blankBrowserAudit(),
      ctx = new EventEmitter();
    let pages = [];
    ctx.pages = () => pages;
    audit.attach(ctx);
    pages = [Object.assign(new EventEmitter(), { url: () => "about:blank" })];
    if (mode === "request") ctx.emit("request", { url: () => "http://127.0.0.1:1234/" });
    const original = fieldNotesOps(m),
      fs = audit.trackFixture(original),
      page = fakePage();
    const input = { scenarios: [scenario], deps: deps({ page, fs }) };
    input.deps.timeouts.attemptMs = 2000;
    const ac = new AbortController();
    if (mode === "abort") ac.abort();
    const identity = (pid, ppid, pgid) => ({
      pid,
      ppid,
      pgid,
      startTicks: String(pid),
      bootId: "boot",
      birthVersion: "linux-proc-v1",
      state: "S",
    });
    const root = identity(3, 2, 3),
      dev = identity(4, 3, 3),
      worker = identity(5, 4, 5);
    let checks = 0,
      settledSample;
    const rows = [];
    const known = new Map([root, dev, worker].map((x) => [x.pid, x]));
    const settling = mode.endsWith("-during-settlement");
    const crossing = mode === "final-cadence-crossing";
    const idleTransient = mode === "idle-transient-in-journal";
    const due = process.hrtime.bigint() + 1000000000n;
    const latest = {
      root,
      dev,
      worker,
      sample: { live: [root, dev], cadenceDeadlineNs: String(due) },
    };
    if (crossing) {
      pages[0].url = () => {
        if (checks === 1) vi.spyOn(process.hrtime, "bigint").mockReturnValue(due);
        return "about:blank";
      };
    }
    const invoke = () =>
      runOriginalFamilyBatch(input, {
        strategy:
          settling || crossing || idleTransient ? "blank-join-owned.v2" : "blank-before-custody.v1",
        browserAudit: audit,
        snapshot: () =>
          mode === "topology-after-journal" && rows.length
            ? [root, dev, identity(8, 4, 8)]
            : [root, dev],
        known: mode === "wrong-population" ? new Map() : known,
        verifyImages() {
          if (mode === "image-after-journal" && rows.length) throw Error("image changed");
        },
        deadlineNs: process.hrtime.bigint() + 10_000_000_000n,
        signal: ac.signal,
        append: (r) => {
          rows.push(r);
          if (idleTransient && r.status === "pre-batch-proof") pages[0].emit("framenavigated");
          if (mode === "navigated-during-journal" && r.status === "pre-batch-proof")
            pages = [{ url: () => "https://example.invalid/" }];
          if (mode === "request-during-journal" && r.status === "pre-batch-proof")
            ctx.emit("request", { url: () => "http://127.0.0.1:1234/" });
        },
        safety: {
          captureFamilyCycle(receipt) {
            if (crossing || idleTransient) return { before: latest, settlement: null };
            const start = String(process.hrtime.bigint());
            const deadline = String(BigInt(start) + 750000000n);
            receipt.cycle = {
              epoch: 3,
              startedNs: start,
              helper: { pid: 8, entryNs: start, spawnHrtimeNs: start, deadlineNs: deadline },
            };
            return {
              before: this.verifyFamilyAdmission(),
              settlement: Promise.resolve().then(() => {
                settledSample = {
                  live: [root, dev],
                  epoch: 3,
                  startedNs: start,
                  helperPid: 8,
                  helperSpawnNs: start,
                  entryNs: start,
                  deadlineNs: deadline,
                  resultNs: String(process.hrtime.bigint()),
                };
                assert.throws(
                  () => (mode.startsWith("snapshot") ? fs.snapshot() : fs.performWrites()),
                  /fixture operation before family admission/,
                );
              }),
            };
          },
          verifyFamilyAdmission() {
            checks++;
            if (crossing || idleTransient) {
              assert(process.hrtime.bigint() < due);
              return latest;
            }
            if (mode === "no-release" || (mode === "fails-after-journal" && checks === 2))
              throw Error("release unavailable");
            return { root, dev, worker, sample: settledSample ?? { live: [root, dev] } };
          },
        },
      });
    if (mode === "released") {
      const result = await invoke();
      const att = input.deps.recorder.records.find((r) => r.type === "attempt");
      assert.equal(att?.mode, "final-state");
      assert.equal(att.writeSteps.length, 2);
      assert(att.writePerformed);
      assert.equal(result.accounting.success, 1);
    } else {
      if (crossing) assert.throws(invoke, /cadence due at family decision/);
      else if (idleTransient) assert.throws(invoke, /left blank setup/);
      else if (settling) await assert.rejects(invoke, /left blank setup/);
      else assert.throws(invoke);
      assert.equal(page.prepared.length, 0);
      assert(
        !input.deps.recorder.records.some((r) =>
          ["attempt", "cleanup", "scenario-start"].includes(r.type),
        ),
      );
      assert.equal(rows.at(-1).admitted, false);
      if (mode === "navigated-during-journal")
        assert.equal(rows.at(-1).pageUrl, "https://example.invalid/");
      if (mode === "request-during-journal") assert.equal(rows.at(-1).httpRequests, 1);
    }
    assert.equal(rows[0].workloadWriteCalls, mode === "write-during-settlement" ? 1 : 0);
    assert.equal(rows[0].snapshots, mode === "snapshot-during-settlement" ? 1 : 0);
  } finally {
    vi.restoreAllMocks();
    rmSync(temp, { recursive: true, force: true });
  }
});
