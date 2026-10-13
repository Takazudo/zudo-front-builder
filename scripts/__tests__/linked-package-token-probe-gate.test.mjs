import { spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, describe, expect, it } from "vite-plus/test";
import { runs, stepIf, workflowStepBlock } from "./helpers/workflow-step-gate.mjs";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const workflow = readFileSync(join(root, ".github/workflows/health.yml"), "utf8");
const stepBlock = (marker) => workflowStepBlock(workflow, marker);
const chromium = stepBlock("name: Install Chromium for scanner boundary acceptance");
const probe = stepBlock("name: Linked-package token identity probe and hydration (#3931)");
const record = stepBlock("name: Record linked-package token probe prerequisite skip (#4200)");
const upload = stepBlock("name: Upload linked-package token probe evidence (#3931)");
const recorderPath = join(root, "scripts/linked-package-token-probe-skip.mjs");
const temporaryDirectories = [];

afterEach(() => {
  for (const directory of temporaryDirectories.splice(0)) {
    rmSync(directory, { recursive: true, force: true });
  }
});

function simulate(state, executedOutcome = "success") {
  const probeRuns = runs(probe, state);
  const outcomes = { ...state.outcomes, linked_probe: probeRuns ? executedOutcome : "skipped" };
  const finalState = { ...state, outcomes, failed: state.failed || executedOutcome === "failure" };
  return {
    probe: probeRuns,
    outcome: outcomes.linked_probe,
    record: runs(record, finalState),
    upload: runs(upload, finalState),
  };
}

// Invoke the real recorder with the outcome/status values wired by health.yml,
// rather than a separate model of its record format or validation behavior.
function invokeRecorder(state, { probeOutcome = "skipped", existingResults } = {}) {
  const directory = mkdtempSync(join(tmpdir(), "linked-probe-gate-"));
  temporaryDirectories.push(directory);
  const resultsPath = join(directory, "results.json");
  const summaryPath = join(directory, "summary.md");
  if (existingResults !== undefined) writeFileSync(resultsPath, existingResults);
  const outcomes = { ...state.outcomes, linked_probe: probeOutcome };
  const jobStatus = state.cancelled ? "cancelled" : state.failed ? "failure" : "success";
  const env = {};
  for (const name of ["PROBE_OUTCOME", "PREREQ_BUILD_BINARY", "PREREQ_CHROMIUM", "JOB_STATUS"]) {
    const value = record.match(new RegExp(`^ {10}${name}: (.+)$`, "m"))?.[1];
    expect(value, `${name} must be wired in the actual recorder step`).toBeTruthy();
    env[name] = value
      .replace(/\$\{\{ steps\.(\w+)\.outcome \}\}/g, (_expression, step) => outcomes[step] ?? "")
      .replace("${{ job.status }}", jobStatus);
    expect(env[name]).not.toContain("${{");
  }
  const invocation = spawnSync(process.execPath, [recorderPath], {
    encoding: "utf8",
    timeout: 2_000,
    env: {
      ...process.env,
      ...env,
      ZFB_PROBE_EVIDENCE_DIR: directory,
      GITHUB_STEP_SUMMARY: summaryPath,
      GITHUB_SHA: "183402a4afe30a68d86b894810be285c9cc54b3b",
      GITHUB_REPOSITORY: "Takazudo/zudo-front-builder",
      GITHUB_RUN_ID: "38359942316",
      GITHUB_RUN_ATTEMPT: "1",
      GITHUB_JOB: "health",
      GITHUB_REF: "refs/pull/4196/merge",
      GITHUB_SERVER_URL: "https://github.com",
    },
  });
  return {
    ...invocation,
    results: existsSync(resultsPath) ? readFileSync(resultsPath, "utf8") : null,
    summary: existsSync(summaryPath) ? readFileSync(summaryPath, "utf8") : null,
  };
}

describe("linked-package probe current-run prerequisite gate (#4200)", () => {
  it("uses stable step ids and current-run outcomes, with failures remaining fatal", () => {
    expect(stepBlock("id: build_binary")).toContain("run: cargo build --workspace --all-targets");
    expect(chromium).toMatch(/^ {8}id: chromium$/m);
    expect(probe).toMatch(/^ {8}id: linked_probe$/m);
    expect(probe).toContain("run: node scripts/linked-package-token-probe.mjs");
    expect(record).toContain("run: node scripts/linked-package-token-probe-skip.mjs");
    expect(stepIf(probe)).toBe(
      "!cancelled() && steps.build_binary.outcome == 'success' && steps.chromium.outcome == 'success'",
    );
    for (const block of [chromium, probe, record]) expect(block).not.toContain("continue-on-error");
  });

  it.each([false, true])("executes with successful prerequisites, prior failure=%s", (failed) => {
    const state = { outcomes: { build_binary: "success", chromium: "success" }, failed };
    expect(runs(chromium, state)).toBe(true);
    expect(simulate(state)).toEqual({
      probe: true,
      outcome: "success",
      record: false,
      upload: true,
    });
  });

  it.each([
    ["failure", "success", ["build_binary"]],
    ["skipped", "success", ["build_binary"]],
    ["success", "failure", ["chromium"]],
    ["success", "skipped", ["chromium"]],
    ["skipped", "skipped", ["build_binary", "chromium"]],
    [undefined, undefined, ["build_binary", "chromium"]],
  ])("records build=%s Chromium=%s as a prerequisite skip", (build, browser, unmet) => {
    const state = { outcomes: { build_binary: build, chromium: browser }, failed: true };
    expect(simulate(state)).toEqual({
      probe: false,
      outcome: "skipped",
      record: true,
      upload: true,
    });
    const actual = invokeRecorder(state);
    expect(actual.status, actual.stderr).toBe(0);
    expect(JSON.parse(actual.results)).toMatchObject({
      status: "skipped",
      reason: "prerequisite-not-met",
      prerequisites: { build_binary: build ?? null, chromium: browser ?? null },
      unmet,
      jobStatus: "failure",
    });
    expect(actual.stdout).toContain("SKIPPED (not passed)");
    expect(actual.summary).toContain("SKIPPED (not passed)");
  });

  it.each(["failure", "skipped", "cancelled", undefined])(
    "does not install Chromium when this run's build outcome is %s",
    (build_binary) => {
      expect(runs(chromium, { outcomes: { build_binary }, failed: true })).toBe(false);
    },
  );

  it("starts no probe/install work on cancellation, with cleanup only attempted", () => {
    const state = {
      outcomes: { build_binary: "success", chromium: "success" },
      cancelled: true,
    };
    expect(runs(chromium, state)).toBe(false);
    expect(simulate(state)).toEqual({
      probe: false,
      outcome: "skipped",
      record: true,
      upload: true,
    });
    const actual = invokeRecorder(state);
    expect(actual.status, actual.stderr).toBe(0);
    expect(JSON.parse(actual.results)).toMatchObject({
      status: "skipped",
      reason: "cancelled",
      unmet: [],
    });
    expect(workflow).toContain("A hard cancel\n      # can interrupt them too");
    // A true upload condition only admits cleanup; it cannot guarantee the
    // cancelled runner completes it. Missing files must remain visible.
    expect(upload).toContain("if-no-files-found: warn");
  });

  it("preserves the early workspace failure and avoids a cascading missing-binary probe", () => {
    const state = { outcomes: { build_binary: "skipped", chromium: "skipped" }, failed: true };
    expect(runs(chromium, state)).toBe(false);
    expect(simulate(state).probe).toBe(false);
    const actual = invokeRecorder(state);
    expect(actual.status, actual.stderr).toBe(0);
    expect(JSON.parse(actual.results)).toMatchObject({ status: "skipped", jobStatus: "failure" });
    expect(actual.results).not.toContain("ENOENT");
    expect(state.failed).toBe(true);
  });

  it("keeps an executed probe failure fatal and leaves its diagnostic bytes intact", () => {
    const state = { outcomes: { build_binary: "success", chromium: "success" } };
    expect(simulate(state, "failure")).toEqual({
      probe: true,
      outcome: "failure",
      record: false,
      upload: true,
    });
    const failedResults = '{"status":"failed","failure":"spawn target/debug/zfb ENOENT"}\n';
    const actual = invokeRecorder(state, {
      probeOutcome: "failure",
      existingResults: failedResults,
    });
    expect(actual.status).not.toBe(0);
    expect(actual.stderr).toContain("outcome is failure, not skipped");
    expect(actual.results).toBe(failedResults);
    expect(actual.summary).toBeNull();
  });

  it.each(["success", "failure", "cancelled"])(
    "never records a prerequisite skip for an executed probe outcome %s",
    (linked_probe) => {
      const state = {
        outcomes: { build_binary: "success", chromium: "success", linked_probe },
        failed: linked_probe === "failure",
        cancelled: linked_probe === "cancelled",
      };
      expect(runs(record, state)).toBe(false);
      expect(runs(upload, state)).toBe(true);
    },
  );

  it("models GitHub's implicit success check for conditions without a status function", () => {
    const state = { outcomes: { build_binary: "success" }, failed: true };
    expect(runs("        if: steps.build_binary.outcome == 'success'", state)).toBe(false);
    expect(runs("        if: !cancelled() && steps.build_binary.outcome == 'success'", state)).toBe(
      true,
    );
  });

  it("refuses to overwrite executed evidence even if called with a skipped outcome", () => {
    const state = { outcomes: { build_binary: "skipped", chromium: "skipped" }, failed: true };
    const failedResults = '{"status":"failed","failure":"hydration identity mismatch"}\n';
    const actual = invokeRecorder(state, { existingResults: failedResults });
    expect(actual.status).not.toBe(0);
    expect(actual.stderr).toContain("EEXIST");
    expect(actual.results).toBe(failedResults);
    expect(actual.summary).toBeNull();
  });

  it("rejects an unjustified skip when prerequisites succeeded despite unrelated failure", () => {
    const state = { outcomes: { build_binary: "success", chromium: "success" }, failed: true };
    const actual = invokeRecorder(state);
    expect(actual.status).not.toBe(0);
    expect(actual.stderr).toContain("every prerequisite succeeded");
    expect(actual.results).toBeNull();
    expect(actual.summary).toBeNull();
  });

  it.each(["success", "cancelled", "", undefined])(
    "rejects recorder execution for probe outcome %s",
    (probeOutcome) => {
      const state = { outcomes: { build_binary: "skipped", chromium: "skipped" }, failed: true };
      const actual = invokeRecorder(state, { probeOutcome: probeOutcome ?? "" });
      expect(actual.status).not.toBe(0);
      expect(actual.stderr).toContain("not skipped");
      expect(actual.results).toBeNull();
    },
  );

  it("records available source/run identity in the same uploaded evidence directory", () => {
    const state = { outcomes: { build_binary: "skipped", chromium: "skipped" }, failed: true };
    const actual = invokeRecorder(state);
    expect(actual.status, actual.stderr).toBe(0);
    expect(JSON.parse(actual.results)).toMatchObject({
      checkoutSha: "183402a4afe30a68d86b894810be285c9cc54b3b",
      run: {
        repository: "Takazudo/zudo-front-builder",
        id: "38359942316",
        attempt: "1",
        job: "health",
        ref: "refs/pull/4196/merge",
        url: "https://github.com/Takazudo/zudo-front-builder/actions/runs/38359942316",
      },
    });
    for (const block of [probe, record]) {
      expect(block).toContain(
        "ZFB_PROBE_EVIDENCE_DIR: ${{ runner.temp }}/linked-package-token-probe",
      );
    }
    expect(upload).toContain("path: ${{ runner.temp }}/linked-package-token-probe/");
    expect(stepIf(upload)).toBe("always()");
  });

  it("retains declared linked-dist Rust coverage and independent browser installations", () => {
    const linkedDist = stepBlock("name: Declared linked dist dev watch row (#3931)");
    expect(stepIf(linkedDist)).toBe("always()");
    for (const name of [
      "e2e_3163_nested_site_compiled_dist_sibling_edit_is_served",
      "e2e_3926_ignored_undeclared_sibling_writes_do_not_tick",
    ]) {
      expect(linkedDist).toContain(
        `cargo test -p zfb --test dev_sibling_watch_1678_e2e ${name} -- --include-ignored`,
      );
    }
    expect(stepBlock("name: Install profile browser engines")).toContain(
      "playwright install --with-deps chromium firefox webkit",
    );
    expect(stepBlock("name: Install Chromium")).toContain(
      "playwright install --with-deps chromium",
    );
  });
});
