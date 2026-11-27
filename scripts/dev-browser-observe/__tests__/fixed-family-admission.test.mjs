import assert from "node:assert/strict";
import { EventEmitter } from "node:events";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "vitest";
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
    pages = [{ url: () => "about:blank" }];
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
    let checks = 0;
    const rows = [];
    const known = new Map([root, dev, worker].map((x) => [x.pid, x]));
    const invoke = () =>
      runOriginalFamilyBatch(input, {
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
          if (mode === "navigated-during-journal" && r.status === "pre-batch-proof")
            pages = [{ url: () => "https://example.invalid/" }];
          if (mode === "request-during-journal" && r.status === "pre-batch-proof")
            ctx.emit("request", { url: () => "http://127.0.0.1:1234/" });
        },
        safety: {
          verifyFamilyAdmission() {
            checks++;
            if (mode === "no-release" || (mode === "fails-after-journal" && checks === 2))
              throw Error("release unavailable");
            return { root, dev, worker, sample: { live: [root, dev] } };
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
      assert.throws(invoke);
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
    assert.equal(rows[0].workloadWriteCalls, 0);
    assert.equal(rows[0].snapshots, 0);
  } finally {
    rmSync(temp, { recursive: true, force: true });
  }
});
