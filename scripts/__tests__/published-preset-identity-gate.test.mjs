import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vite-plus/test";
import {
  FATAL_CONSOLE_TYPES,
  consoleOracleFailures,
  isFatalConsoleMessage,
} from "../../tests/published-preset-identity/console-policy.mjs";
import { skipRecord } from "../../tests/published-preset-identity/record-skip.mjs";
import { runs, stepIf, workflowStepBlock } from "./helpers/workflow-step-gate.mjs";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const workflow = readFileSync(join(root, ".github/workflows/health.yml"), "utf8");

const stepBlock = (marker) => workflowStepBlock(workflow, marker);

const fixture = stepBlock("name: Published-preset runtime identity fixture (#3331)");
const record = stepBlock("name: Record published-preset fixture prerequisite skip (#4177)");
const upload = stepBlock("name: Upload published-preset identity evidence (#3331)");

function simulate(state) {
  const fixtureRuns = runs(fixture, state);
  const outcomes = { ...state.outcomes, published_preset: fixtureRuns ? "success" : "skipped" };
  return {
    fixture: fixtureRuns,
    record: runs(record, { ...state, outcomes }),
    upload: runs(upload, { ...state, outcomes }),
  };
}

describe("published-preset fixture gate in health.yml (#4177)", () => {
  it("names both prerequisite steps the gate depends on", () => {
    expect(stepBlock("id: build_binary")).toContain("run: cargo build --workspace --all-targets");
    expect(stepBlock("name: Install Chromium for scanner boundary acceptance")).toMatch(
      /^ {8}id: chromium$/m,
    );
    expect(fixture).toMatch(/^ {8}id: published_preset$/m);
    expect(fixture).toContain("ZFB_BIN: ${{ github.workspace }}/target/debug/zfb");
  });

  it("keeps the fixture a hard failure: no continue-on-error, no always()", () => {
    for (const block of [fixture, record]) expect(block).not.toContain("continue-on-error");
    expect(stepIf(fixture)).not.toContain("always()");
  });

  it("runs when its prerequisites succeeded", () => {
    const outcomes = { build_binary: "success", chromium: "success" };
    expect(simulate({ outcomes })).toEqual({ fixture: true, record: false, upload: true });
  });

  it("still runs after an unrelated earlier failure", () => {
    const outcomes = { build_binary: "success", chromium: "success" };
    expect(simulate({ outcomes, failed: true })).toEqual({
      fixture: true,
      record: false,
      upload: true,
    });
  });

  it.each([
    ["the binary build failed", { build_binary: "failure", chromium: "success" }],
    ["the binary build was skipped", { build_binary: "skipped", chromium: "success" }],
    ["the Chromium install failed", { build_binary: "success", chromium: "failure" }],
  ])("records an explicit skip when %s", (_label, outcomes) => {
    expect(simulate({ outcomes, failed: true })).toEqual({
      fixture: false,
      record: true,
      upload: true,
    });
  });

  it("records an explicit skip on cancellation", () => {
    const outcomes = { build_binary: "success", chromium: "success" };
    expect(simulate({ outcomes, cancelled: true })).toEqual({
      fixture: false,
      record: true,
      upload: true,
    });
  });
});

describe("published-preset skip record (#4177)", () => {
  it("names the unmet prerequisite and is never a pass", () => {
    const value = skipRecord({
      outcomes: { build_binary: "failure", chromium: "success" },
      jobStatus: "failure",
    });
    expect(value).toMatchObject({
      status: "skipped",
      reason: "prerequisite-not-met",
      unmet: ["build_binary"],
      prerequisites: { build_binary: "failure", chromium: "success" },
    });
  });

  it("distinguishes cancellation", () => {
    const value = skipRecord({
      outcomes: { build_binary: "success", chromium: "success" },
      jobStatus: "cancelled",
    });
    expect(value).toMatchObject({ status: "skipped", reason: "cancelled", unmet: [] });
  });

  it("refuses to record a skip the gate should not have produced", () => {
    expect(() =>
      skipRecord({
        outcomes: { build_binary: "success", chromium: "success" },
        jobStatus: "failure",
      }),
    ).toThrow(/every prerequisite succeeded/);
  });
});

describe("published-preset positive console oracle (#4177)", () => {
  it("treats warnings as fatal alongside errors", () => {
    expect(FATAL_CONSOLE_TYPES).toEqual(["error", "warning"]);
    expect(isFatalConsoleMessage("warning")).toBe(true);
    expect(isFatalConsoleMessage("error")).toBe(true);
    for (const type of ["log", "info", "debug"]) expect(isFatalConsoleMessage(type)).toBe(false);
  });

  it("fails on a warning, an error or a page error", () => {
    expect(consoleOracleFailures({ console: ["warning: deprecated"], pageErrors: [] })).toEqual([
      "console errors/warnings: warning: deprecated",
    ]);
    expect(consoleOracleFailures({ console: ["error: boom"], pageErrors: [] })).toHaveLength(1);
    expect(consoleOracleFailures({ console: [], pageErrors: ["TypeError"] })).toEqual([
      "page errors: TypeError",
    ]);
    expect(consoleOracleFailures({ console: [], pageErrors: [] })).toEqual([]);
  });

  it("is wired into the harness with no allowlist", () => {
    const harness = readFileSync(join(root, "tests/published-preset-identity/run.mjs"), "utf8");
    expect(harness).toContain("isFatalConsoleMessage(message.type())");
    expect(harness).toContain("failures.push(...consoleOracleFailures(facts));");
    expect(harness).not.toMatch(/allowlist|ignoreWarning/i);
  });
});
