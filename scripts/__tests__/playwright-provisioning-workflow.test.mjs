import { spawnSync } from "node:child_process";
import { chmodSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, describe, expect, it } from "vite-plus/test";
import { parseArgs } from "../install-playwright-linux.mjs";
import { runs, stepIf, workflowStepBlock } from "./helpers/workflow-step-gate.mjs";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const health = readFileSync(join(root, ".github/workflows/health.yml"), "utf8");
const smoke = readFileSync(join(root, ".github/workflows/node-free-smoke.yml"), "utf8");
const healthStep = (marker) => workflowStepBlock(health, marker);

function jobBlock(workflow, id) {
  const start = workflow.indexOf(`\n  ${id}:\n`);
  expect(start, `job ${id} not found`).toBeGreaterThanOrEqual(0);
  const next = workflow.slice(start + 1).search(/\n {2}[a-z0-9-]+:\n/);
  return workflow.slice(start + 1, next === -1 ? undefined : start + 1 + next);
}

function stepIn(job, marker) {
  const start = job.indexOf(`      - ${marker}\n`);
  expect(start, `step ${marker} not found`).toBeGreaterThanOrEqual(0);
  const next = job.indexOf("\n      - ", start + 1);
  return job.slice(start, next === -1 ? undefined : next);
}

function runScript(block) {
  const single = block.match(/^ {8}run: (?!\|)(.+)$/m);
  if (single) return single[1];
  const lines = block.split("\n");
  const start = lines.findIndex((line) => /^ {8}run: \|$/.test(line));
  expect(start).toBeGreaterThanOrEqual(0);
  const body = [];
  for (const line of lines.slice(start + 1)) {
    if (line.trim() !== "" && !line.startsWith("          ")) break;
    body.push(line.slice(10));
  }
  return body.join("\n");
}

function helperArgs(command) {
  const match = command.match(/^node scripts\/install-playwright-linux\.mjs (.+)$/);
  expect(match, `not a helper invocation: ${command}`).not.toBeNull();
  return parseArgs(match[1].split(/\s+/));
}

// The helper clips every phase to --total-timeout and stops issuing work
// early enough to save its receipt, so the total is its worst case.
const worstCaseSeconds = (options) => options.totalTimeoutMs / 1000;

const temporary = [];
afterEach(() => {
  for (const directory of temporary.splice(0)) rmSync(directory, { recursive: true, force: true });
});

// Executes the step's real shell with a fake `node` that records its argv.
function executeWithFakeNode(script, env, nodeExit = 0) {
  const directory = mkdtempSync(join(tmpdir(), "provision-step-"));
  temporary.push(directory);
  const log = join(directory, "node-args");
  const fake = join(directory, "node");
  writeFileSync(fake, `#!/bin/sh\nprintf '%s\\n' "$*" >> "${log}"\nexit ${nodeExit}\n`);
  chmodSync(fake, 0o755);
  const result = spawnSync("bash", ["-e", "-c", script], {
    encoding: "utf8",
    env: { PATH: `${directory}:/usr/bin:/bin`, ...env },
  });
  let calls = [];
  try {
    calls = readFileSync(log, "utf8").trim().split("\n").filter(Boolean);
  } catch {
    calls = [];
  }
  return { status: result.status, stderr: result.stderr, calls };
}

describe("health Chromium stage", () => {
  const chromium = healthStep("name: Install Chromium for scanner boundary acceptance");

  it("keeps id, current-run build prerequisite and hard failure", () => {
    expect(chromium).toMatch(/^ {8}id: chromium$/m);
    expect(stepIf(chromium)).toBe("!cancelled() && steps.build_binary.outcome == 'success'");
    expect(chromium).not.toContain("continue-on-error");
    expect(helperArgs(runScript(chromium)).engines).toEqual(["chromium"]);
  });

  it("runs after an unrelated earlier failure but not after cancellation or a failed build", () => {
    expect(runs(chromium, { outcomes: { build_binary: "success" }, failed: true })).toBe(true);
    expect(runs(chromium, { outcomes: { build_binary: "success" }, cancelled: true })).toBe(false);
    for (const build_binary of ["failure", "skipped", "cancelled", undefined]) {
      expect(runs(chromium, { outcomes: { build_binary }, failed: true })).toBe(false);
    }
  });

  it("is consumed only through its own outcome, before any later browser stage", () => {
    const wind = health.indexOf("      - name: Install profile browser engines\n");
    const consumers = [
      "name: Scanner boundary acceptance (Chromium)",
      "name: Linked-package token identity probe and hydration (#3931)",
      "name: Published-preset runtime identity fixture (#3331)",
    ];
    for (const marker of consumers) {
      expect(health.indexOf(`      - ${marker}\n`)).toBeLessThan(wind);
    }
    for (const marker of consumers.slice(1)) {
      const condition = stepIf(healthStep(marker));
      expect(condition).toBe(
        "!cancelled() && steps.build_binary.outcome == 'success' && steps.chromium.outcome == 'success'",
      );
      // Chromium ready, then any later (e.g. Firefox/WebKit) failure: the
      // consumer has already been decided by its own prerequisites.
      expect(
        runs(healthStep(marker), {
          outcomes: { build_binary: "success", chromium: "success" },
          failed: true,
        }),
      ).toBe(true);
      expect(
        runs(healthStep(marker), {
          outcomes: { build_binary: "success", chromium: "failure" },
          failed: true,
        }),
      ).toBe(false);
    }
  });
});

describe("health Wind stage", () => {
  const wind = healthStep("name: Install profile browser engines");
  const script = runScript(wind);

  it("keeps its route gate and owns no Chromium-stage id", () => {
    expect(stepIf(wind)).toBe("steps.wind_route.outputs.relevant == 'true'");
    expect(wind).not.toMatch(/^ {8}id:/m);
    expect(wind).toContain("WIND_MODE: ${{ steps.wind_route.outputs.mode }}");
    expect(wind).not.toContain("continue-on-error");
  });

  it.each([
    ["full", "health-wind-full", ["chromium", "firefox", "webkit"]],
    ["chromium", "health-wind-chromium", ["chromium"]],
  ])("routes %s mode to the helper with the profile engines", (mode, label, engines) => {
    const result = executeWithFakeNode(script, { WIND_MODE: mode });
    expect(result.status).toBe(0);
    expect(result.calls).toHaveLength(1);
    const options = helperArgs(`node ${result.calls[0]}`);
    expect(options.label).toBe(label);
    expect(options.engines).toEqual(engines);
  });

  it.each(["", "unknown", "FULL"])("fails closed on route mode %j without provisioning", (mode) => {
    const result = executeWithFakeNode(script, { WIND_MODE: mode });
    expect(result.status).not.toBe(0);
    expect(result.calls).toEqual([]);
    expect(result.stderr).toContain("unknown Wind route mode");
  });

  it("propagates a helper failure as a step failure", () => {
    expect(executeWithFakeNode(script, { WIND_MODE: "full" }, 1).status).not.toBe(0);
  });

  it("keeps island-size collection on its own prerequisites", () => {
    expect(stepIf(healthStep("name: Build and pack packages for island-size measurement"))).toBe(
      "!cancelled() && steps.build_binary.outcome == 'success' && steps.size_toolchain.outcome == 'success'",
    );
  });
});

describe("provisioning budgets and receipts", () => {
  it("keeps the admitted runners and outer deadlines", () => {
    const healthJob = jobBlock(health, "health");
    expect(healthJob).toMatch(/^ {4}runs-on: ubuntu-24\.04$/m);
    expect(healthJob).toMatch(/^ {4}timeout-minutes: 90$/m);
    const smokeJob = jobBlock(smoke, "built-site-browser-smoke");
    expect(smokeJob).toMatch(/^ {4}runs-on: ubuntu-24\.04$/m);
    expect(smokeJob).toMatch(/^ {4}timeout-minutes: 10$/m);
  });

  it("fits every helper's worst case inside its job deadline", () => {
    const healthJob = jobBlock(health, "health");
    const healthWorst =
      worstCaseSeconds(
        helperArgs(runScript(healthStep("name: Install Chromium for scanner boundary acceptance"))),
      ) +
      worstCaseSeconds(
        helperArgs(
          runScript(healthStep("name: Install profile browser engines"))
            .split("\n")
            .map((line) => line.trim())
            .find((line) => line.includes("health-wind-full")),
        ),
      );
    // Healthy main runs spend about 31-36 minutes on everything else.
    expect(healthWorst + 36 * 60).toBeLessThan(90 * 60);
    expect(healthJob).toContain("name: playwright-provisioning-health");

    const smokeJob = jobBlock(smoke, "built-site-browser-smoke");
    const smokeOptions = helperArgs(runScript(stepIn(smokeJob, "name: Install Chromium")));
    expect(smokeOptions.engines).toEqual(["chromium"]);
    // About 30 s before the step and 15 s after it on healthy runs.
    expect(worstCaseSeconds(smokeOptions) + 60).toBeLessThan(10 * 60);

    const wasmJob = jobBlock(health, "wasm-md-browser");
    const wasmOptions = helperArgs(runScript(stepIn(wasmJob, "name: Install Chromium")));
    expect(wasmOptions.engines).toEqual(["chromium"]);
    expect(worstCaseSeconds(wasmOptions) + 3 * 60).toBeLessThan(20 * 60);
  });

  it("uploads receipts even when provisioning or tests fail", () => {
    for (const [workflow, job, name] of [
      [health, "health", "playwright-provisioning-health"],
      [health, "wasm-md-browser", "playwright-provisioning-wasm-md-browser"],
      [smoke, "built-site-browser-smoke", "playwright-provisioning-built-site-smoke"],
    ]) {
      const block = jobBlock(workflow, job);
      const upload = block.slice(
        block.indexOf(`name: ${name}`) - 400,
        block.indexOf(`name: ${name}`) + 200,
      );
      expect(upload).toMatch(/if: always\(\)/);
      expect(upload).toContain("path: ${{ runner.temp }}/playwright-provisioning/");
    }
  });

  it("leaves no direct --with-deps install in the repaired jobs", () => {
    for (const block of [
      jobBlock(health, "health"),
      jobBlock(health, "wasm-md-browser"),
      jobBlock(smoke, "built-site-browser-smoke"),
    ]) {
      expect(block).not.toContain("--with-deps");
    }
  });
});
