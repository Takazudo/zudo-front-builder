// Synthetic orchestration and saved-byte fixtures; not native measurements.
import { spawnSync } from "node:child_process";
import { existsSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, describe, expect, it } from "vite-plus/test";
import { parse } from "yaml";
import {
  assertCompletePair,
  budgetAllowsRetention,
  runBudgetEvidence,
} from "../island-size-evidence.mjs";
import { ceilingsOf, makeCheckout, write, writeRuns } from "./fixtures/island-size-checkout.mjs";
const root = fileURLToPath(new URL("../..", import.meta.url));
const workflow = parse(readFileSync(resolve(root, ".github/workflows/health.yml"), "utf8"));
const roots = [];
afterEach(() => roots.splice(0).forEach((p) => rmSync(p, { recursive: true, force: true })));
const sourceSha = "b".repeat(40);
const fixtures = [
  "no-island",
  "event-only",
  "scalar-signal",
  "show-for",
  "model",
  "blog-theme",
  "json-api",
  "multi-island",
];
const rejection =
  "Island size budget rejected:\n" +
  [
    "local Cargo.lock: SHA-256 differs from the reviewed contract",
    "workspace provenance: Cargo lockfile differs from the contract",
    "packed provenance: Cargo lockfile differs from the contract",
  ]
    .map((e) => "- " + e + "\n")
    .join("");
function savedPair() {
  const { root } = makeCheckout(roots);
  const directory = resolve(root, "target/island-size");
  writeRuns(directory, fixtures, (mode) => ({ sourceSha, mode }));
  for (const mode of ["workspace", "packed"])
    write(directory, "real-" + mode + "/report.md", "Synthetic fixture, not native evidence.\n");
  const contract = { ceilings: ceilingsOf(directory) };
  write(root, "research/v3-island-size/decision-linux-x64.json", JSON.stringify(contract));
  return { root, directory, contract, output: resolve(root, "github-output") };
}
// Execute actual YAML conditions with GitHub's implicit success() default.
// Only expensive commands/actions are replaced by synthetic outcome fixtures.
function simulate(overrides = {}, cancelBefore) {
  const steps = {};
  let failed = false,
    cancelled = false;
  const seen = [];
  for (const step of workflow.jobs.health.steps) {
    const id = step.id ?? step.name ?? step.run ?? step.uses;
    if (id === cancelBefore) cancelled = true;
    const expression = String(step.if ?? "success()").replace(/^\$\{\{\s*|\s*\}\}$/g, "");
    const relevant =
      !step.if || /steps\.(native_prerequisites|build_binary|size_)/.test(expression);
    let run = !failed && !cancelled;
    if (relevant && step.if) {
      const explicit = /\b(success|failure|cancelled|always)\(/.test(expression);
      run =
        (explicit || (!failed && !cancelled)) &&
        Function(
          "steps",
          "cancelled",
          "success",
          "failure",
          "always",
          "return (" + expression + ")",
        )(
          steps,
          () => cancelled,
          () => !failed && !cancelled,
          () => failed,
          () => true,
        );
    }
    if (!relevant) run = false;
    const outcome = run ? (overrides[id] ?? "success") : "skipped";
    if (run) seen.push(id);
    if (outcome === "failure") failed = true;
    if (outcome === "cancelled") cancelled = true;
    if (step.id)
      steps[step.id] = {
        outcome,
        outputs: step.id === "size_budget" ? { complete: overrides.complete ?? "true" } : {},
      };
  }
  return { steps, seen, result: cancelled ? "cancelled" : failed ? "failure" : "success" };
}
describe("Linux collection orchestration (synthetic outcomes)", () => {
  it("collects after early tests fail without making required Health green", () => {
    const run = simulate({ workspace_tests: "failure" });
    expect(run.steps.build_binary.outcome).toBe("success");
    expect(run.steps.size_measurements.outcome).toBe("success");
    expect(run.seen).toContain("Upload island-size measurement evidence");
    expect(run.result).toBe("failure");
    expect(
      workflow.jobs.health.steps.find((s) => s.id === "workspace_tests")["continue-on-error"],
    ).toBeUndefined();
  });
  for (const prerequisite of [
    "pnpm install --frozen-lockfile",
    "native_prerequisites",
    "build_binary",
    "size_node",
    "size_toolchain",
    "size_packages",
    "size_measurements",
  ]) {
    it("withholds evidence after " + prerequisite + " fails", () => {
      const run = simulate({ [prerequisite]: "failure" });
      expect(run.seen).not.toContain("Upload island-size measurement evidence");
      expect(run.result).toBe("failure");
    });
  }
  for (const id of [
    "build_binary",
    "size_node",
    "size_toolchain",
    "size_packages",
    "size_measurements",
    "size_budget",
  ]) {
    it("does not start " + id + " after cancellation", () => {
      const run = simulate({}, id);
      expect(run.steps[id].outcome).toBe("skipped");
      expect(run.seen).not.toContain("Upload island-size measurement evidence");
      expect(run.result).toBe("cancelled");
    });
  }
  it("does not run linked-watch native tests on cancellation or failed build", () => {
    const name = "Declared linked dist dev watch row (#3931)";
    expect(simulate({}, name).seen).not.toContain(name);
    expect(simulate({ build_binary: "failure" }).seen).not.toContain(name);
  });
  it("retains old-pin-only evidence while red, but withholds other failures", () => {
    const run = simulate({ size_budget: "failure" });
    expect(run.seen).toContain("Upload island-size measurement evidence");
    expect(run.result).toBe("failure");
    expect(simulate({ size_budget: "failure", complete: "" }).seen).not.toContain(
      "Upload island-size measurement evidence",
    );
  });
  it("executes the real stale-output cleanup before packing", () => {
    const script = workflow.jobs.health.steps.find((s) => s.id === "size_packages").run;
    const { root } = makeCheckout(roots);
    write(root, "target/island-size/real-workspace/stale", "stale");
    const result = spawnSync("bash", ["-e", "-c", script.slice(0, script.indexOf("pnpm -C"))], {
      cwd: root,
      env: { ...process.env, GITHUB_WORKSPACE: root },
      encoding: "utf8",
    });
    expect(result.status, result.stderr).toBe(0);
    expect(existsSync(resolve(root, "target/island-size/real-workspace/stale"))).toBe(false);
  });
});
describe("saved-byte eligibility (synthetic fixtures)", () => {
  it("retains exact old-pin rejection and preserves subprocess exit/output", () => {
    const pair = savedPair();
    let diagnostic = "";
    const status = runBudgetEvidence({
      ...pair,
      sourceSha,
      check: () =>
        spawnSync(
          process.execPath,
          ["-e", "process.stderr.write(" + JSON.stringify(rejection) + ");process.exit(1)"],
          { encoding: "utf8" },
        ),
      log: { write() {} },
      errors: {
        write(s) {
          diagnostic += s;
        },
      },
    });
    expect(status).toBe(1);
    expect(diagnostic).toBe(rejection);
    expect(readFileSync(pair.output, "utf8")).toBe("complete=true\n");
  });
  it("refuses extra diagnostics and abnormal exits", () => {
    for (const result of [
      { status: 1, stderr: rejection + "- unrelated identity mismatch\n" },
      { status: 2, stderr: rejection },
      { status: null, signal: "SIGTERM", stderr: rejection },
      { status: 0, error: new Error("spawn failure"), stderr: "" },
    ])
      expect(budgetAllowsRetention(result)).toBe(false);
  });
  for (const kind of [
    "missing row",
    "stale source",
    "altered sidecar",
    "stale output",
    "increase",
    "decrease",
  ]) {
    it("rejects " + kind, () => {
      const pair = savedPair();
      const dir = resolve(pair.directory, "real-workspace");
      if (kind === "missing row") rmSync(resolve(dir, "event-only"), { recursive: true });
      if (kind === "stale output") write(dir, "unexpected", "stale");
      if (kind === "altered sidecar") {
        const p = resolve(dir, "measurement.normalized.json");
        const sidecar = JSON.parse(readFileSync(p, "utf8"));
        sidecar.results["event-only"].shipped.raw++;
        writeFileSync(p, JSON.stringify(sidecar));
      }
      if (kind === "increase") pair.contract.ceilings.workspace["event-only"].raw--;
      if (kind === "decrease") pair.contract.ceilings.workspace["event-only"].raw++;
      expect(() =>
        assertCompletePair(
          pair.directory,
          pair.contract,
          kind === "stale source" ? "c".repeat(40) : sourceSha,
        ),
      ).toThrow();
    });
  }
});
