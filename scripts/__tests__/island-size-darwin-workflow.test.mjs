import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vite-plus/test";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const workflow = readFileSync(join(root, ".github/workflows/island-size-darwin.yml"), "utf8");

const jobStart = workflow.indexOf("\n  measure-darwin:");
const linuxStart = workflow.indexOf("\n  measure-linux:");
const darwin = workflow.slice(jobStart, linuxStart);
const linux = workflow.slice(linuxStart);

function expectOrdered(job) {
  const measure = job.indexOf("measure-real.mjs");
  const normalize = job.indexOf("normalize-identity.mjs");
  const check = job.indexOf("check-budget.mjs");
  expect(measure).toBeGreaterThan(-1);
  expect(normalize).toBeGreaterThan(measure);
  expect(check).toBeGreaterThan(normalize);
  expect(job).toContain('--measurement "$out/real-$mode/measurement.json"');
  expect(job).toContain("for mode in workspace packed");
}

describe("island-size native workflow wiring", () => {
  it("is dispatch-only with no obsolete PR bootstrap", () => {
    expect(workflow).toContain("workflow_dispatch:");
    expect(workflow).not.toMatch(/^\s*pull_request:/m);
    expect(workflow).not.toMatch(/^\s*push:/m);
    expect(workflow).not.toContain("sweep-261007-2");
  });

  it("keeps the registered filename and displays as island-size-native", () => {
    expect(workflow).toMatch(/^name: island-size-native$/m);
  });

  it("declares the platform choice input defaulting to darwin-arm64", () => {
    expect(workflow).toMatch(
      /platform:\n\s+description: .+\n\s+required: false\n\s+default: darwin-arm64\n\s+type: choice\n\s+options:\n\s+- darwin-arm64\n\s+- linux-x64\n\s+- both\n/,
    );
  });

  it("gates each job on inputs.platform", () => {
    expect(darwin).toContain("if: inputs.platform == 'darwin-arm64' || inputs.platform == 'both'");
    expect(linux).toContain("if: inputs.platform == 'linux-x64' || inputs.platform == 'both'");
    expect(workflow).not.toMatch(/^ {2}measure:/m);
  });

  it("normalizes both measurements before the budget check in each job", () => {
    expectOrdered(darwin);
    expectOrdered(linux);
  });

  it("keeps the Darwin native guard, artifact name and failure-time upload", () => {
    expect(darwin).toContain('"Darwin:arm64"');
    expect(darwin).toContain(
      "name: island-size-darwin-arm64-${{ steps.context.outputs.source_sha }}",
    );
    expect(darwin).toMatch(/Upload complete Darwin island-size evidence\s+if: always\(\)/);
    expect(darwin).toContain("path: target/island-size/");
    expect(darwin).not.toContain("--contract");
  });

  it("runs the Linux lane natively against the Linux contract", () => {
    expect(linux).toContain("runs-on: ubuntu-24.04");
    expect(linux).toContain('"Linux:x86_64"');
    expect(linux).toContain('process.platform !== "linux" || process.arch !== "x64"');
    expect(linux).toContain("--contract research/v3-island-size/decision-linux-x64.json");
    expect(linux).toContain('prefix-key: "v1-island-size-linux"');
    expect(linux).not.toContain("shared-key");
    expect(linux).not.toContain("test:workspace");
    expect(linux).not.toContain("Darwin");
  });

  it("uploads Linux evidence on failure too", () => {
    expect(linux).toMatch(/Upload complete Linux x64 island-size evidence\s+if: always\(\)/);
    expect(linux).toContain("name: island-size-linux-x64-${{ steps.context.outputs.source_sha }}");
    expect(linux).toContain("path: target/island-size/");
  });
});
